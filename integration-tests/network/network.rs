//! The network adapters, against recorded responses and a mock endpoint.
//!
//! # Why a mock endpoint rather than a recorded struct
//!
//! A test that fed the adapter an `HorizonTransaction` it had constructed itself would
//! prove that the engine can read its own types. What it would not prove is that the
//! adapter reads the bytes Horizon actually sends - and that is where an endpoint that
//! moved, a field that was renamed or a pagination cursor that changed would be felt.
//!
//! So the recordings in `fixtures/transactions/` and `fixtures/ledgers/` are served over
//! a real HTTP socket by `wiremock`, and the adapter's own client fetches and parses
//! them. The request path the adapter builds is asserted too, because a client that
//! fetched the right shape from the wrong path would pass every other check here.
//!
//! # What is deliberately not here
//!
//! No live network. A test that reached testnet would be non-deterministic, would fail
//! for reasons unrelated to the change under review, and would be the test a contributor
//! learns to ignore. The live smoke test is `scripts/test-testnet.sh`, which runs on a
//! schedule and asserts only that the commands reach a real endpoint.

use amasario_core::{EngineConfig, ErrorCategory};
use amasario_integration_tests::corpus::{Corpus, assert_committed};
use amasario_integration_tests::recordings;
use amasario_network::horizon::{Collection, HorizonLedger};
use amasario_network::{
    HorizonSession, HorizonTransaction, NetworkTarget, classify_status, status_is_absent,
    status_is_transient,
};
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Every recording is committed, and what is committed is what the corpus defines.
#[test]
fn every_recording_matches_the_corpus() {
    for recording in recordings::all() {
        assert_committed(recording.directory, recording.file, &recording.rendered());
    }
}

/// A Horizon transaction page parses into the adapter's own record type.
#[test]
fn the_recorded_transaction_page_parses_as_horizon_transactions() {
    let text = Corpus::read("transactions", recordings::HORIZON_TRANSACTIONS.file);
    let page: Collection<HorizonTransaction> =
        serde_json::from_str(&text).expect("the recorded page is a Horizon collection");

    assert_eq!(page.embedded.records.len(), 2);
    let first = &page.embedded.records[0];
    let second = &page.embedded.records[1];

    assert_eq!(first.hash, recordings::horizon_transaction_hashes()[0]);
    assert_eq!(first.ledger, Some(1_043));
    assert_eq!(first.successful, Some(true), "the first call succeeded");

    // The second record is the one that matters: a failed transaction is a fact about
    // the chain, and the adapter must carry it rather than assume success.
    assert_eq!(
        second.successful,
        Some(false),
        "the recorded page must hold a failed transaction, or the distinction is untested"
    );
    assert!(
        second.created_at.is_some(),
        "Horizon's timestamps must survive parsing, because a temporal model needs them"
    );
}

/// A Horizon ledger record parses into the adapter's own record type.
#[test]
fn the_recorded_ledger_parses_as_a_horizon_ledger() {
    let text = Corpus::read("ledgers", recordings::HORIZON_LEDGER.file);
    let ledger: HorizonLedger = serde_json::from_str(&text).expect("the recorded ledger parses");

    assert_eq!(ledger.sequence, 1_044);
    assert_eq!(ledger.protocol_version, Some(22));
    assert_eq!(ledger.successful_transaction_count, Some(1));
    assert_eq!(ledger.failed_transaction_count, Some(1));
    assert_eq!(
        ledger.transaction_count,
        Some(2),
        "the counts must add up, or the recording describes a ledger that cannot exist"
    );
}

/// A record with an unknown field still parses.
///
/// Horizon is a separate service with its own release cadence. A field the engine does
/// not understand must be ignored, not rejected: an analysis that failed because that
/// service added a field would be an analysis that broke without anything being wrong.
#[test]
fn an_unknown_field_in_a_recorded_response_is_tolerated() {
    let mut value: Value = serde_json::from_str(&Corpus::read(
        "transactions",
        recordings::HORIZON_TRANSACTIONS.file,
    ))
    .expect("the recording parses");
    value["_embedded"]["records"][0]["a_field_horizon_added_later"] = Value::from(42);

    let page: Collection<HorizonTransaction> =
        serde_json::from_value(value).expect("an added field must not break the adapter");
    assert_eq!(page.embedded.records.len(), 2);
}

/// The RPC network-identity recording identifies testnet, and the other does not.
///
/// The check the engine performs before it analyses anything: a wrong endpoint must be
/// refused rather than producing an analysis of one chain labelled as another.
#[test]
fn the_recorded_network_identities_are_different_chains() {
    let testnet = recordings::RPC_NETWORK_TESTNET.value();
    let futurenet = recordings::RPC_NETWORK_FUTURENET.value();

    let testnet_passphrase = testnet["result"]["passphrase"]
        .as_str()
        .expect("a passphrase");
    let futurenet_passphrase = futurenet["result"]["passphrase"]
        .as_str()
        .expect("a passphrase");

    assert_eq!(
        testnet_passphrase,
        amasario_integration_tests::documents::TESTNET_PASSPHRASE
    );
    assert_ne!(
        testnet_passphrase, futurenet_passphrase,
        "the two recordings must be different chains, or the refusal they exercise is \
         vacuous"
    );

    // The engine's own identifier for a passphrase is what the check compares, so the
    // two must not collide.
    let left = amasario_network::network_id_for_passphrase(testnet_passphrase);
    let right = amasario_network::network_id_for_passphrase(futurenet_passphrase);
    assert_ne!(left, right);
}

/// The recorded latest-ledger response answers the trait a boundary is read from.
#[test]
fn the_recorded_latest_ledger_carries_a_ledger_sequence() {
    let value = recordings::RPC_LATEST_LEDGER.value();
    let sequence = value["result"]["sequence"]
        .as_u64()
        .expect("a ledger sequence");
    assert_eq!(
        sequence,
        u64::from(amasario_integration_tests::documents::BOUNDARY_LEDGER)
    );
    assert!(
        value["result"]["id"].is_string(),
        "the endpoint reports the ledger's own hash, which is what makes the boundary \
         identifiable rather than merely numbered"
    );
}

/// The adapter fetches the page over a real socket and parses it.
///
/// The end-to-end path: a URL is built, an HTTP request is made, the bytes are read and
/// the adapter's own deserialiser runs on them. Nothing here is stubbed except the
/// server.
#[tokio::test]
async fn the_adapter_fetches_and_parses_a_recorded_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/accounts/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM/transactions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(Corpus::read(
            "transactions",
            recordings::HORIZON_TRANSACTIONS.file,
        )))
        .expect(1)
        .mount(&server)
        .await;

    let target = NetworkTarget::custom(
        "mock",
        amasario_integration_tests::documents::TESTNET_PASSPHRASE,
        &format!("{}/soroban/rpc", server.uri()),
        Some(&server.uri()),
        &EngineConfig::default(),
    )
    .expect("a custom target");

    let session = HorizonSession::connect(&target).expect("a Horizon session");
    let page: Vec<HorizonTransaction> = session
        .collection(
            &amasario_core::Cancellation::new(),
            "/accounts/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM/transactions",
            2,
            None,
        )
        .await
        .expect("the page is fetched and parsed");

    assert_eq!(page.len(), 2);
    assert_eq!(
        page.iter()
            .map(|record| record.hash.clone())
            .collect::<Vec<_>>(),
        recordings::horizon_transaction_hashes(),
        "the adapter read the same hashes the recording holds"
    );
    assert_eq!(page[1].successful, Some(false));
}

/// A `404` from the endpoint becomes an absence rather than a failure.
#[tokio::test]
async fn an_absent_resource_is_classified_as_absent_not_as_a_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/accounts/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM/transactions",
        ))
        .respond_with(
            ResponseTemplate::new(404).set_body_string("{\"title\":\"Resource Missing\"}"),
        )
        .mount(&server)
        .await;

    let target = NetworkTarget::custom(
        "mock",
        amasario_integration_tests::documents::TESTNET_PASSPHRASE,
        &format!("{}/soroban/rpc", server.uri()),
        Some(&server.uri()),
        &EngineConfig::default(),
    )
    .expect("a custom target");

    let session = HorizonSession::connect(&target).expect("a Horizon session");
    let result: Result<Vec<HorizonTransaction>, _> = session
        .collection(
            &amasario_core::Cancellation::new(),
            "/accounts/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM/transactions",
            2,
            None,
        )
        .await;

    let error = result.expect_err("a 404 is not a page of transactions");
    assert_eq!(
        error.category(),
        ErrorCategory::Network,
        "an absence is still reported through the network layer: {error}"
    );
    assert!(
        status_is_absent(404),
        "the status vocabulary must classify this as an absence"
    );
    assert!(
        !status_is_transient(404),
        "an absence is not worth retrying, and treating it as transient is how a tool \
         retries forever"
    );
}

/// A server-side error is transient and therefore retried.
///
/// The opposite case, and the reason both are here: a tool that treated a `503` as an
/// absence would report a contract as nonexistent because a server was busy.
#[tokio::test]
async fn a_transient_failure_is_classified_as_worth_retrying() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(503).set_body_string("{\"title\":\"Service Unavailable\"}"),
        )
        .mount(&server)
        .await;

    let target = NetworkTarget::custom(
        "mock",
        amasario_integration_tests::documents::TESTNET_PASSPHRASE,
        &format!("{}/soroban/rpc", server.uri()),
        Some(&server.uri()),
        &EngineConfig {
            // One attempt, because the assertion is about the classification rather
            // than about the retry: a test that waited out a backoff would be slow and
            // would say the same thing.
            retry: amasario_core::RetryPolicy {
                max_attempts: 1,
                ..EngineConfig::default().retry
            },
            ..EngineConfig::default()
        },
    )
    .expect("a custom target");

    let session = HorizonSession::connect(&target).expect("a Horizon session");
    let result: Result<Vec<HorizonTransaction>, _> = session
        .collection(
            &amasario_core::Cancellation::new(),
            "/accounts/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM/transactions",
            2,
            None,
        )
        .await;

    let error = result.expect_err("a 503 is not a page of transactions");
    assert_eq!(error.category(), ErrorCategory::Network);
    assert!(status_is_transient(503));
    assert!(!status_is_absent(503));
    assert!(
        error.to_string().contains("503")
            || error.to_string().to_lowercase().contains("unavailable"),
        "the classification must name what the endpoint said: {error}"
    );
}

/// A malformed body is a defect, and the reported error says so.
#[tokio::test]
async fn a_body_that_is_not_json_is_reported_as_a_defect() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(recordings::MALFORMED_BODY.body))
        .mount(&server)
        .await;

    let target = NetworkTarget::custom(
        "mock",
        amasario_integration_tests::documents::TESTNET_PASSPHRASE,
        &format!("{}/soroban/rpc", server.uri()),
        Some(&server.uri()),
        &EngineConfig::default(),
    )
    .expect("a custom target");

    let session = HorizonSession::connect(&target).expect("a Horizon session");
    let result: Result<Vec<HorizonTransaction>, _> = session
        .collection(
            &amasario_core::Cancellation::new(),
            "/accounts/CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM/transactions",
            2,
            None,
        )
        .await;

    let error = result.expect_err("a truncated body is not a page of transactions");
    // A malformed body is a network-layer failure, and the engine keeps it distinct from
    // an absence even though both arrive over the same transport. The stronger check is
    // that it is not reported as a missing contract, which is the misreport that would
    // turn a decoding bug into a confident claim about the chain.
    assert_eq!(error.category(), ErrorCategory::Network);
    assert!(
        !matches!(error, amasario_core::EngineError::ContractNotFound { .. }),
        "a malformed response must not be reported as an absent contract"
    );
    assert_eq!(error.code(), "AMASARIO_NETWORK_MALFORMED_RESPONSE");
}

/// A Horizon endpoint has to be configured rather than inferred.
///
/// The endpoint discipline: operation history lives on Horizon, and the engine does not
/// guess at its address from the RPC endpoint's.
#[test]
fn a_target_without_a_horizon_endpoint_refuses_to_build_a_session() {
    let target = NetworkTarget::custom(
        "mock",
        amasario_integration_tests::documents::TESTNET_PASSPHRASE,
        "https://rpc.example.test",
        None,
        &EngineConfig::default(),
    )
    .expect("a custom target");
    let error = HorizonSession::connect(&target).expect_err("no Horizon endpoint is configured");
    assert_eq!(error.category(), ErrorCategory::Configuration);
    assert!(
        error.to_string().contains("Horizon"),
        "the refusal must name what is missing: {error}"
    );
}

/// The status vocabulary keeps all four combinations of absent and transient apart.
#[test]
fn the_status_vocabulary_covers_every_combination() {
    let statuses = recordings::statuses();
    let mut seen_absent = false;
    let mut seen_transient = false;
    let mut seen_permanent = false;

    for recorded in statuses {
        assert_eq!(status_is_absent(recorded.status), recorded.absent);
        assert_eq!(status_is_transient(recorded.status), recorded.transient);
        seen_absent |= recorded.absent;
        seen_transient |= recorded.transient;
        seen_permanent |= !recorded.transient && !recorded.absent;
    }

    assert!(seen_absent, "the corpus must hold an absence");
    assert!(seen_transient, "the corpus must hold a transient failure");
    assert!(
        seen_permanent,
        "the corpus must hold a rejected request, which is neither"
    );

    // And the classification of each is a network failure with the status in it.
    for recorded in recordings::statuses() {
        let error = classify_status(
            "https://horizon.example.test",
            recorded.status,
            recorded.detail,
        );
        assert!(
            error.to_string().contains(&recorded.status.to_string()),
            "{error}"
        );
    }
}
