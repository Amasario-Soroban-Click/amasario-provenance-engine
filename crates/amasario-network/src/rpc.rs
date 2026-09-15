//! The RPC transport: retry, health, ledger entries, events and transactions.
//!
//! This module owns the interaction with Stellar RPC. It does not interpret what
//! it receives - that is the job of [`crate::contracts`],
//! [`crate::events`] and [`crate::transactions`] - but it does own three
//! responsibilities that every caller depends on.
//!
//! # 1. Retry, driven by classification
//!
//! [`RpcSession::attempt`] retries exactly the failures
//! [`crate::errors::classify`] marked retryable, and nothing else. The number of
//! attempts and the backoff come from
//! [`RetryPolicy`], so two runs configured
//! identically behave identically rather than depending on wall-clock luck.
//!
//! # 2. Network identity is verified before anything is observed
//!
//! A passphrase mismatch is checked once, up front, and is fatal. If it were
//! checked lazily, an analysis could accumulate observations from one chain and
//! then be labelled with another - which is the single most confusing output this
//! tool could produce, and one no downstream check could detect.
//!
//! # 3. Absence is not failure
//!
//! [`RpcSession::transaction`] returns `Ok(None)` when the network says the
//! transaction does not exist, and an error when the network could not be asked.
//! A caller can therefore always distinguish "the network says no" from "the
//! network did not answer".
//!
//! # Re-exported types
//!
//! The response types below are `stellar-rpc-client`'s own. Re-exporting them
//! rather than translating them keeps the engine's vocabulary aligned with the
//! protocol's; a translation layer would be a place for the two to drift.

use std::future::Future;
use std::time::Duration;

use amasario_core::{Cancellation, EngineError, LedgerSequence, Result, RetryPolicy};
use stellar_rpc_client::{
    Client, Error as RpcError, EventStart, EventType, GetEventsResponse, GetLedgersResponse,
    GetTransactionResponse, LedgerStart, TopicFilter,
};
use stellar_xdr::LedgerKey;
use tracing::{debug, warn};

use crate::client::{NetworkTarget, RpcEndpoint};
use crate::errors;

pub use stellar_rpc_client::{Event, FullLedgerEntry as LedgerEntry, Ledger};

/// The default number of events requested per page.
///
/// Kept modest because a page is a single response held in memory, and because a
/// smaller page makes a rate limit easier to back off from. Callers that need a
/// larger page pass an explicit limit; the endpoint may impose its own cap, which
/// is why the scan follows the returned cursor rather than assuming the page was
/// filled.
pub const DEFAULT_EVENT_PAGE_LIMIT: usize = 100;

/// What an endpoint reports about its own state.
///
/// `oldest_ledger` is the field that makes an event scan safe: the documented
/// behaviour of `getEvents` is to fail when `startLedger` falls outside what the
/// node retains, so the scan is bounded by this value rather than discovering the
/// boundary through a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeStatus {
    /// The most recent ledger the endpoint has seen.
    pub latest_ledger: LedgerSequence,
    /// The oldest ledger the endpoint retains.
    pub oldest_ledger: LedgerSequence,
    /// How many ledgers the endpoint retains.
    pub ledger_retention_window: u32,
}

/// The identity of the network an endpoint serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkIdentity {
    /// The passphrase the endpoint reported.
    pub passphrase: String,
    /// The network id, derived as the SHA-256 of that passphrase.
    pub network_id: String,
    /// The protocol version the endpoint implements.
    pub protocol_version: u32,
}

/// A connection to one Stellar RPC endpoint.
#[derive(Debug, Clone)]
pub struct RpcSession {
    endpoint: RpcEndpoint,
    client: Client,
    retry: RetryPolicy,
}

impl RpcSession {
    /// Connects to an endpoint, applying the target's retry policy.
    ///
    /// The per-attempt timeout is taken from
    /// [`RetryPolicy::timeout`](amasario_core::RetryPolicy::timeout) so that the
    /// timeout a caller configured is the timeout that applies, rather than a
    /// library default they cannot see.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] when the endpoint cannot be used to
    /// construct a client, which happens for a URL that is well-formed enough to
    /// pass [`RpcEndpoint`] but not a valid HTTP base.
    pub fn connect(target: &NetworkTarget) -> Result<Self> {
        let endpoint = target.rpc().clone();
        let retry = *target.retry();

        // The per-attempt timeout is applied by [`bounded`] rather than by the
        // client. That is deliberate: the client's own timeout constructor is
        // deprecated in favour of one that takes headers and no timeout at all, so
        // relying on it would either emit a deprecation warning or silently lose
        // the timeout. Applying it here also makes the timeout a property of the
        // engine's retry policy, which is where a caller configured it, rather than
        // a library default they cannot see.
        let client = Client::new(endpoint.as_str())
            .map_err(|error| errors::classify(endpoint.as_str(), &error))?;

        Ok(Self {
            endpoint,
            client,
            retry,
        })
    }

    /// The endpoint being used.
    #[must_use]
    pub const fn endpoint(&self) -> &RpcEndpoint {
        &self.endpoint
    }

    /// Runs `operation`, retrying the failures that are worth retrying.
    ///
    /// The attempt count is bounded by the policy, so this can never become an
    /// unbounded retry loop. Cancellation is polled before every attempt, so a
    /// cancelled run stops at a request boundary and can report that it stopped
    /// rather than appearing to have completed.
    ///
    /// # Errors
    ///
    /// Returns the last failure when the attempts are exhausted, and the first
    /// non-retryable failure immediately. A cancelled run returns the cancellation
    /// error rather than a transport error, because the two mean different things.
    pub async fn attempt<T, F, Fut>(
        &self,
        cancellation: &Cancellation,
        operation: &str,
        run: F,
    ) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 1_u32;

        loop {
            cancellation.check()?;

            match run().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    if !error.retryable() || !self.retry.may_retry_after(attempt) {
                        if self.retry.may_retry_after(attempt) {
                            debug!(
                                operation,
                                attempt,
                                reason = %error,
                                "not retrying: the failure is not transient"
                            );
                        }
                        return Err(error);
                    }

                    let delay = self.retry.delay_for_attempt(attempt);
                    let next = attempt + 1;
                    warn!(
                        operation,
                        attempt,
                        of = self.retry.max_attempts,
                        delay_ms = delay.as_millis(),
                        reason = %error,
                        "retrying a transient failure"
                    );

                    // The delay is interruptible so that cancelling a run does not
                    // wait out a backoff it no longer intends to use.
                    if !delay.is_zero() {
                        let sleep = tokio::time::sleep(delay);
                        tokio::select! {
                            () = sleep => {}
                            () = wait_for_cancellation(cancellation) => {
                                cancellation.check()?;
                            }
                        }
                    }

                    attempt = next;
                },
            }
        }
    }

    /// Asks the endpoint about its own state.
    ///
    /// # Errors
    ///
    /// Returns a classified [`EngineError`] when the endpoint cannot be reached or
    /// the response is unusable. Never returns a default, because a fabricated
    /// node status would bound an analysis by a ledger nobody observed.
    pub async fn node_status(&self, cancellation: &Cancellation) -> Result<NodeStatus> {
        let client = self.client.clone();
        let endpoint = self.endpoint.as_str().to_owned();
        let timeout = self.retry.timeout;

        let response = self
            .attempt(cancellation, "getHealth", || {
                let client = client.clone();
                let endpoint = endpoint.clone();
                async move { bounded(timeout, &endpoint, client.get_health()).await }
            })
            .await?;

        Ok(NodeStatus {
            latest_ledger: LedgerSequence::new(response.latest_ledger)?,
            oldest_ledger: LedgerSequence::new(response.oldest_ledger)?,
            ledger_retention_window: response.ledger_retention_window,
        })
    }

    /// Verifies that the endpoint serves the expected network and reports its
    /// identity.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError::Network`] naming the mismatch when the endpoint
    /// serves a different chain. This is permanent: no amount of retrying changes
    /// which network an endpoint serves.
    pub async fn network_identity(
        &self,
        cancellation: &Cancellation,
        expected_passphrase: &str,
    ) -> Result<NetworkIdentity> {
        let endpoint = self.endpoint.as_str().to_owned();
        let client = self.client.clone();

        // One request, not two: the comparison is made here rather than delegated
        // to the client's own passphrase check so that a mismatch can be reported
        // with the engine's own classification and a message that explains what the
        // mismatch means for the analysis.
        let timeout = self.retry.timeout;
        let response = self
            .attempt(cancellation, "getNetwork", || {
                let client = client.clone();
                let endpoint = endpoint.clone();
                async move { bounded(timeout, &endpoint, client.get_network()).await }
            })
            .await?;

        if response.passphrase != expected_passphrase {
            return Err(EngineError::Network {
                endpoint,
                detail: format!(
                    "the endpoint serves {:?} but {:?} was requested; retrying cannot change which \
                     network an endpoint serves",
                    response.passphrase, expected_passphrase
                ),
                retryable: false,
            });
        }

        Ok(NetworkIdentity {
            network_id: crate::client::network_id_for_passphrase(&response.passphrase),
            passphrase: response.passphrase,
            protocol_version: response.protocol_version,
        })
    }

    /// The most recent ledger the endpoint has seen.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the endpoint cannot be reached, and a
    /// validation error if it reports a ledger number of zero, which no Stellar
    /// network has because the genesis ledger is 1.
    pub async fn latest_ledger(&self, cancellation: &Cancellation) -> Result<LedgerSequence> {
        let client = self.client.clone();
        let endpoint = self.endpoint.as_str().to_owned();
        let timeout = self.retry.timeout;

        let response = self
            .attempt(cancellation, "getLatestLedger", || {
                let client = client.clone();
                let endpoint = endpoint.clone();
                async move { bounded(timeout, &endpoint, client.get_latest_ledger()).await }
            })
            .await?;

        LedgerSequence::new(response.sequence)
    }

    /// Reads ledger entries by key, with each entry's own last-modified ledger.
    ///
    /// An empty result means the keys are absent at the current ledger, which is a
    /// definite answer. A failure to ask is an error. The distinction is the whole
    /// point of the return type.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails or a returned entry cannot
    /// be decoded as XDR.
    pub async fn ledger_entries(
        &self,
        cancellation: &Cancellation,
        keys: &[LedgerKey],
    ) -> Result<Vec<LedgerEntry>> {
        let client = self.client.clone();
        let endpoint = self.endpoint.as_str().to_owned();
        let timeout = self.retry.timeout;
        let keys = keys.to_vec();

        // The keys are cloned per attempt rather than borrowed, so that the future
        // returned by the closure owns everything it needs. A borrowed future would
        // tie each retry to the caller's stack frame, which would make the retry
        // loop's lifetime requirements depend on the caller rather than on here.
        let response = self
            .attempt(cancellation, "getLedgerEntries", || {
                let client = client.clone();
                let endpoint = endpoint.clone();
                let keys = keys.clone();
                async move { bounded(timeout, &endpoint, client.get_full_ledger_entries(&keys)).await }
            })
            .await?;

        Ok(response.entries)
    }

    /// Reads one page of events.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails. A page with no events is
    /// a successful empty page, not a failure, and the cursor in the response is
    /// what tells a caller whether more remain.
    pub async fn events_page(
        &self,
        cancellation: &Cancellation,
        start: EventStart,
        contract_ids: &[String],
        limit: usize,
    ) -> Result<GetEventsResponse> {
        let event_type = Some(EventType::Contract);
        let topics: Vec<TopicFilter> = Vec::new();
        let client = self.client.clone();
        let endpoint = self.endpoint.as_str().to_owned();
        let timeout = self.retry.timeout;
        let contract_ids = contract_ids.to_vec();

        self.attempt(cancellation, "getEvents", || {
            let client = client.clone();
            let endpoint = endpoint.clone();
            let start = start.clone();
            let contract_ids = contract_ids.clone();
            let topics = topics.clone();
            async move {
                bounded(
                    timeout,
                    &endpoint,
                    client.get_events(start, event_type, &contract_ids, &topics, Some(limit)),
                )
                .await
            }
        })
        .await
    }

    /// Reads a transaction by hash.
    ///
    /// Returns `Ok(None)` when the network reports the transaction as not found,
    /// so that absence stays distinguishable from failure.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails or the response cannot be
    /// decoded.
    pub async fn transaction(
        &self,
        cancellation: &Cancellation,
        hash: &stellar_xdr::Hash,
    ) -> Result<Option<GetTransactionResponse>> {
        let hash = hash.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.as_str().to_owned();
        let timeout = self.retry.timeout;

        let response = self
            .attempt(cancellation, "getTransaction", || {
                let client = client.clone();
                let endpoint = endpoint.clone();
                let hash = hash.clone();
                async move { bounded(timeout, &endpoint, client.get_transaction(&hash)).await }
            })
            .await?;

        // Stellar RPC reports an unknown transaction as a successful response whose
        // status is `NOT_FOUND`. Treating that as an error would conflate "the
        // network says this transaction does not exist" with "the network could not
        // be asked".
        if response.status.eq_ignore_ascii_case("NOT_FOUND") {
            return Ok(None);
        }

        Ok(Some(response))
    }

    /// Reads ledger headers and metadata for a range of ledgers.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails. A range outside the
    /// endpoint's retention window is a failure, and callers should bound against
    /// [`RpcSession::node_status`] rather than discovering that here.
    pub async fn ledgers(
        &self,
        cancellation: &Cancellation,
        start: LedgerSequence,
        limit: usize,
    ) -> Result<GetLedgersResponse> {
        let start_ledger = start.get();
        let client = self.client.clone();
        let endpoint = self.endpoint.as_str().to_owned();
        let timeout = self.retry.timeout;

        self.attempt(cancellation, "getLedgers", || {
            let client = client.clone();
            let endpoint = endpoint.clone();
            async move {
                bounded(
                    timeout,
                    &endpoint,
                    client.get_ledgers(LedgerStart::Ledger(start_ledger), Some(limit), None),
                )
                .await
            }
        })
        .await
    }
}

/// Runs one attempt of an RPC call under the configured per-attempt timeout.
///
/// The timeout is applied here rather than by the client so that the value a
/// caller configured in
/// [`RetryPolicy::timeout`](amasario_core::RetryPolicy::timeout) is the value that
/// applies. A timeout is classified as a transient network failure, because a
/// request that did not complete in time says nothing about whether the endpoint
/// would answer a later one.
async fn bounded<T, Fut>(timeout: Duration, endpoint: &str, call: Fut) -> Result<T>
where
    Fut: Future<Output = std::result::Result<T, RpcError>>,
{
    match tokio::time::timeout(timeout, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(errors::classify(endpoint, &error)),
        Err(_elapsed) => Err(EngineError::transient_network(
            endpoint,
            format!(
                "the request did not complete within {}s",
                timeout.as_secs().max(1)
            ),
        )),
    }
}

/// Resolves when cancellation has been requested.
///
/// Polled rather than notified because [`Cancellation`] is a flag, not a channel:
/// making it a channel would require every holder to be able to wake a waiter,
/// which is more machinery than a cooperative stop needs.
async fn wait_for_cancellation(cancellation: &Cancellation) {
    while !cancellation.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The wait between attempts, exposed so a caller can reason about the worst case.
///
/// Returns the total time a run could spend sleeping across all retries under a
/// policy, which is what a CI timeout has to accommodate.
#[must_use]
pub fn worst_case_retry_wait(policy: &RetryPolicy) -> Duration {
    let mut total = Duration::ZERO;
    for attempt in 1..policy.max_attempts {
        total = total.saturating_add(policy.delay_for_attempt(attempt));
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{failure, result};
    use amasario_core::{EngineConfig, NetworkType};
    use std::time::Duration;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    fn fast_policy() -> RetryPolicy {
        RetryPolicy {
            timeout: Duration::from_secs(5),
            max_attempts: 3,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
        }
    }

    fn target(server: &MockServer) -> NetworkTarget {
        let config = EngineConfig {
            retry: fast_policy(),
            ..EngineConfig::default()
        };
        NetworkTarget::custom(
            "mock",
            "Test SDF Network ; September 2015",
            &server.uri(),
            None,
            &config,
        )
        .expect("a local endpoint is valid")
    }

    /// Mounts a responder for the given JSON-RPC method.
    ///
    /// The responder echoes the request's own identifier. The client rejects a
    /// response whose identifier does not match the call it is waiting on, so a
    /// hardcoded identifier would fail for reasons unrelated to the behaviour under
    /// test.
    async fn mount(server: &MockServer, method_name: &str, body: impl Respond + 'static) {
        Mock::given(method("POST"))
            .and(body_string_contains(method_name))
            .respond_with(body)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn the_latest_ledger_is_read_from_the_endpoint() {
        let server = MockServer::start().await;
        mount(
            &server,
            "getLatestLedger",
            result(serde_json::json!({
                "id": "abc",
                "protocolVersion": 23,
                "sequence": 1_234_567
            })),
        )
        .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let ledger = session
            .latest_ledger(&Cancellation::new())
            .await
            .expect("reads the ledger");
        assert_eq!(ledger.get(), 1_234_567);
    }

    #[tokio::test]
    async fn node_status_reports_the_retention_window_it_was_told() {
        // The retention window is what bounds an event scan, so it must be read
        // rather than assumed.
        let server = MockServer::start().await;
        mount(
            &server,
            "getHealth",
            result(serde_json::json!({
                "status": "healthy",
                "latestLedger": 900,
                "oldestLedger": 400,
                "ledgerRetentionWindow": 500
            })),
        )
        .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let status = session
            .node_status(&Cancellation::new())
            .await
            .expect("reads status");
        assert_eq!(status.latest_ledger.get(), 900);
        assert_eq!(status.oldest_ledger.get(), 400);
        assert_eq!(status.ledger_retention_window, 500);
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_and_then_succeeds() {
        let server = MockServer::start().await;

        // Fail once with a server-side condition, then answer properly. A
        // server-side condition is the class the classification marks retryable.
        Mock::given(method("POST"))
            .and(body_string_contains("getLatestLedger"))
            .respond_with(failure(-32000, "busy"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;

        mount(
            &server,
            "getLatestLedger",
            result(serde_json::json!({"id": "abc", "protocolVersion": 23, "sequence": 42})),
        )
        .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let ledger = session
            .latest_ledger(&Cancellation::new())
            .await
            .expect("succeeds on the second attempt");
        assert_eq!(ledger.get(), 42);
    }

    #[tokio::test]
    async fn a_rejected_request_is_not_retried() {
        let server = MockServer::start().await;

        // `-32602` says the request was rejected, so repeating it is pointless. The
        // mock asserts exactly one request was made, which is the assertion that
        // proves the classification actually drives behaviour.
        Mock::given(method("POST"))
            .and(body_string_contains("getLatestLedger"))
            .respond_with(failure(-32602, "invalid params"))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = session
            .latest_ledger(&Cancellation::new())
            .await
            .expect_err("a rejected request fails");
        assert!(!error.retryable());
        assert!(error.to_string().contains("-32602"), "got: {error}");
    }

    #[tokio::test]
    async fn a_retryable_failure_still_fails_once_the_attempts_are_exhausted() {
        let server = MockServer::start().await;

        // Three attempts are permitted, so exactly three requests must be made: a
        // retry loop that ignored the bound would exceed this, and one that never
        // retried would not reach it.
        Mock::given(method("POST"))
            .and(body_string_contains("getLatestLedger"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .expect(3)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = session
            .latest_ledger(&Cancellation::new())
            .await
            .expect_err("the endpoint never recovers");
        assert!(error.retryable());
    }

    #[tokio::test]
    async fn a_malformed_response_fails_without_retrying() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(body_string_contains("getLatestLedger"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = session
            .latest_ledger(&Cancellation::new())
            .await
            .expect_err("a malformed body is a defect");
        assert!(!error.retryable(), "got: {error}");
    }

    #[tokio::test]
    async fn a_missing_contract_data_key_returns_an_empty_result_rather_than_an_error() {
        // Absence is a definite answer. Returning an error here would make a
        // caller unable to tell "the entry is absent" from "the query failed".
        let server = MockServer::start().await;
        mount(
            &server,
            "getLedgerEntries",
            result(serde_json::json!({ "entries": null, "latestLedger": 100 })),
        )
        .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let entries = session
            .ledger_entries(&Cancellation::new(), &[])
            .await
            .expect("absence is not a failure");
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn an_unknown_transaction_is_reported_as_absent_rather_than_as_an_error() {
        let server = MockServer::start().await;
        mount(
            &server,
            "getTransaction",
            result(serde_json::json!({ "status": "NOT_FOUND" })),
        )
        .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let found = session
            .transaction(&Cancellation::new(), &stellar_xdr::Hash([0_u8; 32]))
            .await
            .expect("the network answered");
        assert!(
            found.is_none(),
            "the network said the transaction does not exist"
        );
    }

    #[tokio::test]
    async fn a_transaction_that_exists_is_returned_with_its_status() {
        let server = MockServer::start().await;
        mount(
            &server,
            "getTransaction",
            result(serde_json::json!({
                "status": "SUCCESS",
                "ledger": 77,
                "txHash": "00"
            })),
        )
        .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let found = session
            .transaction(&Cancellation::new(), &stellar_xdr::Hash([0_u8; 32]))
            .await
            .expect("the network answered")
            .expect("the transaction exists");
        assert_eq!(found.status, "SUCCESS");
        assert_eq!(found.ledger, Some(77));
    }

    #[tokio::test]
    async fn a_cancelled_run_stops_instead_of_retrying() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(body_string_contains("getLatestLedger"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .mount(&server)
            .await;

        let cancellation = Cancellation::new();
        cancellation.cancel();

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = session
            .latest_ledger(&cancellation)
            .await
            .expect_err("a cancelled run must not proceed");
        assert!(
            error.to_string().contains("cancelled"),
            "the reason must be cancellation, not a transport failure: {error}"
        );
    }

    #[test]
    fn the_worst_case_retry_wait_is_bounded_and_computed_from_the_policy() {
        let policy = RetryPolicy {
            timeout: Duration::from_secs(5),
            max_attempts: 4,
            backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
        };
        // Two retries after the first attempt: 100ms then 200ms.
        assert_eq!(worst_case_retry_wait(&policy), Duration::from_millis(300));

        let single = RetryPolicy {
            max_attempts: 1,
            ..policy
        };
        assert_eq!(worst_case_retry_wait(&single), Duration::ZERO);
    }

    #[test]
    fn an_endpoint_reports_the_network_its_target_identified() {
        let config = EngineConfig::default();
        let target = NetworkTarget::custom(
            "private",
            "Test SDF Network ; September 2015",
            "https://rpc.example.test",
            None,
            &config,
        )
        .expect("valid");
        assert_eq!(target.network().network_type, NetworkType::Custom);
        assert!(target.rpc().as_str().starts_with("https://"));
    }

    #[tokio::test]
    async fn a_passphrase_mismatch_is_reported_and_not_retried() {
        // Analysing one chain and labelling the result with another is the most
        // confusing output the tool could produce, so the check is up front and
        // fatal.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getNetwork"))
            .respond_with(result(serde_json::json!({
                "passphrase": "Public Global Stellar Network ; September 2015",
                "protocolVersion": 23
            })))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = session
            .network_identity(&Cancellation::new(), "Test SDF Network ; September 2015")
            .await
            .expect_err("the endpoint serves a different network");
        assert!(!error.retryable());
        assert!(
            error.to_string().contains("cannot change which network"),
            "got: {error}"
        );
    }
}
