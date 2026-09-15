//! The Horizon REST adapter.
//!
//! # Why the engine has two transports
//!
//! Stellar RPC is the right transport for anything expressed in ledger entries and
//! events: it is the interface the network implements for contract state, and its
//! answers are XDR. Horizon is the right transport for *history*: a transaction's
//! operations, an account's activity, a ledger's record - the things a REST
//! collection exposes with paging that RPC does not.
//!
//! The two are not interchangeable, and the engine does not treat them as such. A
//! fact read from Horizon is labelled with the same
//! [`ObservationBoundary`](amasario_core::ObservationBoundary) as one read from
//! RPC, because what makes an observation reproducible is the boundary, not the
//! transport.
//!
//! # Failure handling
//!
//! Response bodies are read as text before parsing. A malformed body is then
//! reported with the bytes that were received, which is the difference between a
//! diagnosable defect and "JSON error at line 1". A `404` is returned as
//! `Ok(None)` rather than an error, because "Horizon does not have this
//! transaction" is a definite answer and collapsing it into a transport failure
//! would make an absence look like an inability to check.

use std::time::Duration;

use amasario_core::{Cancellation, EngineError, Result, RetryPolicy};
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::warn;
use url::Url;

use crate::client::{HorizonEndpoint, NetworkTarget};
use crate::errors;

/// A transaction record as Horizon reports it.
///
/// Every field is optional and unknown fields are tolerated, deliberately: Horizon
/// is a separate service with its own release cadence, and the engine must not fail
/// an analysis because that service added a field. Fields the engine does not
/// understand are ignored rather than rejected - the opposite of the policy for the
/// engine's *own* document formats, where an unknown field is a version
/// incompatibility and rejecting it is the safe answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HorizonTransaction {
    /// The transaction hash, lowercase hex.
    pub hash: String,
    /// The ledger the transaction was included in.
    #[serde(default)]
    pub ledger: Option<u32>,
    /// Whether the transaction succeeded.
    #[serde(default)]
    pub successful: Option<bool>,
    /// When the transaction was created, as an RFC 3339 timestamp.
    #[serde(default)]
    pub created_at: Option<String>,
    /// The transaction's source account.
    #[serde(default)]
    pub source_account: Option<String>,
    /// The number of operations the transaction contains.
    #[serde(default)]
    pub operation_count: Option<u32>,
    /// The transaction envelope, base64 XDR.
    #[serde(default)]
    pub envelope_xdr: Option<String>,
    /// The transaction result, base64 XDR.
    #[serde(default)]
    pub result_xdr: Option<String>,
    /// The transaction result metadata, base64 XDR.
    #[serde(default)]
    pub result_meta_xdr: Option<String>,
    /// The fee charged, as Horizon reports it.
    #[serde(default)]
    pub fee_charged: Option<String>,
    /// Whether the envelope is a fee-bump envelope.
    #[serde(default)]
    pub fee_bump: Option<bool>,
}

/// A ledger record as Horizon reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HorizonLedger {
    /// The ledger sequence.
    pub sequence: u32,
    /// The ledger hash.
    #[serde(default)]
    pub hash: Option<String>,
    /// The number of transactions in the ledger.
    #[serde(default)]
    pub transaction_count: Option<u32>,
    /// The number of successful transactions in the ledger.
    #[serde(default)]
    pub successful_transaction_count: Option<u32>,
    /// The number of failed transactions in the ledger.
    #[serde(default)]
    pub failed_transaction_count: Option<u32>,
    /// The ledger's close time, as an RFC 3339 timestamp.
    #[serde(default)]
    pub closed_at: Option<String>,
    /// The ledger's protocol version.
    #[serde(default)]
    pub protocol_version: Option<u32>,
}

/// Horizon's paged-collection envelope.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Collection<T> {
    /// The records in this page.
    #[serde(rename = "_embedded")]
    pub embedded: CollectionRecords<T>,
}

/// The records inside a Horizon collection.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CollectionRecords<T> {
    /// The page's records.
    pub records: Vec<T>,
}

/// A connection to one Horizon endpoint.
#[derive(Debug, Clone)]
pub struct HorizonSession {
    endpoint: HorizonEndpoint,
    client: reqwest::Client,
    retry: RetryPolicy,
}

impl HorizonSession {
    /// Connects to the target's Horizon endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] when the target has no Horizon
    /// endpoint, or when one cannot be used to build an HTTP client. A missing
    /// endpoint is an error rather than an empty session because a caller that
    /// asked for history would otherwise silently receive nothing.
    pub fn connect(target: &NetworkTarget) -> Result<Self> {
        let endpoint = target.horizon().cloned().ok_or_else(|| {
            EngineError::Configuration(format!(
                "no Horizon endpoint is configured for {}; operation and transaction history \
                 requires one, and it is not inferred from the RPC endpoint",
                target.network().id
            ))
        })?;

        let client = reqwest::Client::builder()
            .timeout(target.retry().timeout)
            .user_agent(concat!("amasario/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                EngineError::Configuration(format!(
                    "could not build an HTTP client for {endpoint}: {error}"
                ))
            })?;

        Ok(Self {
            endpoint,
            client,
            retry: *target.retry(),
        })
    }

    /// The endpoint being used.
    #[must_use]
    pub const fn endpoint(&self) -> &HorizonEndpoint {
        &self.endpoint
    }

    /// Builds a URL under the endpoint's root.
    ///
    /// The path's leading slash is optional and is dropped before the join. Without
    /// that, a caller that wrote `\"/accounts/G.../transactions\"` - which is how Horizon
    /// documents the route - would produce `base//accounts/...`, and a double slash is
    /// not the same route: a server that does not normalise it answers `404`, which the
    /// adapter would otherwise report as an absent resource rather than as the URL
    /// mistake it is.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Network`] when the endpoint and path do not join into
    /// a valid URL, which can only happen if the endpoint was configured unusually.
    fn url(&self, path: &str, query: &[(&str, String)]) -> Result<Url> {
        let base = self.endpoint.as_str().trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let mut url = Url::parse(&format!("{base}/{path}")).map_err(|error| {
            EngineError::permanent_network(
                self.endpoint.as_str(),
                format!("could not build a request URL: {error}"),
            )
        })?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    /// Fetches and decodes one resource.
    ///
    /// Returns `Ok(None)` when the endpoint reports the resource as absent, and an
    /// error otherwise. This is the single place the distinction is made, so every
    /// lookup in this module inherits it.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails, the status is an error,
    /// or the body cannot be decoded.
    pub async fn get<T: DeserializeOwned>(
        &self,
        cancellation: &Cancellation,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<T>> {
        let url = self.url(path, query)?;
        let endpoint = self.endpoint.as_str().to_owned();
        let mut attempt = 1_u32;

        loop {
            cancellation.check()?;

            let outcome = self
                .client
                .get(url.clone())
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await;

            let error = match outcome {
                Err(error) => {
                    // A timeout or a connection failure is transient; anything else
                    // the transport reports is reported as permanent, because
                    // retrying a rejected request changes nothing.
                    if error.is_timeout() || error.is_connect() || error.is_request() {
                        EngineError::transient_network(&endpoint, error.to_string())
                    } else {
                        EngineError::permanent_network(&endpoint, error.to_string())
                    }
                },
                Ok(response) => {
                    let status = response.status();
                    if status == StatusCode::NOT_FOUND {
                        return Ok(None);
                    }
                    let body = response.text().await.map_err(|error| {
                        EngineError::permanent_network(
                            &endpoint,
                            format!("could not read the response body: {error}"),
                        )
                    })?;

                    if status.is_success() {
                        return serde_json::from_str::<T>(&body).map(Some).map_err(|error| {
                            EngineError::MalformedResponse {
                                endpoint: endpoint.clone(),
                                detail: format!(
                                    "the body was not the expected JSON: {error}; received: {}",
                                    truncate_for_message(&body)
                                ),
                            }
                        });
                    }

                    let detail = horizon_error_detail(&body);
                    errors::classify_status(&endpoint, status.as_u16(), &detail)
                },
            };

            if !error.retryable() || !self.retry.may_retry_after(attempt) {
                return Err(error);
            }

            let delay = self.retry.delay_for_attempt(attempt);
            warn!(
                endpoint = %endpoint,
                attempt,
                of = self.retry.max_attempts,
                delay_ms = delay.as_millis(),
                reason = %error,
                "retrying a transient Horizon failure"
            );

            if !delay.is_zero() {
                // Interruptible so that cancelling does not wait out a backoff it no
                // longer intends to use.
                let sleep = tokio::time::sleep(delay);
                tokio::select! {
                    () = sleep => {}
                    () = wait_for_cancellation(cancellation) => cancellation.check()?,
                }
            }

            attempt += 1;
        }
    }

    /// Reads a transaction by hash.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails or the response cannot be
    /// decoded.
    pub async fn transaction(
        &self,
        cancellation: &Cancellation,
        hash: &str,
    ) -> Result<Option<HorizonTransaction>> {
        self.get(cancellation, &format!("transactions/{hash}"), &[])
            .await
    }

    /// Reads a ledger by sequence.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the request fails or the response cannot be
    /// decoded.
    pub async fn ledger(
        &self,
        cancellation: &Cancellation,
        sequence: u32,
    ) -> Result<Option<HorizonLedger>> {
        self.get(cancellation, &format!("ledgers/{sequence}"), &[])
            .await
    }

    /// Reads one page of a collection.
    ///
    /// `limit` is bounded rather than passed through: a caller that asks for more
    /// records than the endpoint allows would otherwise receive a silently smaller
    /// page, and a page size the engine cannot predict is one it cannot reason
    /// about when deciding whether to continue.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] for a zero or over-large limit, and a
    /// classified error when the request fails.
    pub async fn collection<T: DeserializeOwned>(
        &self,
        cancellation: &Cancellation,
        path: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Vec<T>> {
        if limit == 0 {
            return Err(EngineError::Configuration(
                "a collection limit of zero would request nothing".to_owned(),
            ));
        }
        if limit > MAX_COLLECTION_LIMIT {
            return Err(EngineError::Configuration(format!(
                "a collection limit of {limit} exceeds the bound of {MAX_COLLECTION_LIMIT}"
            )));
        }

        let mut query: Vec<(&str, String)> =
            vec![("limit", limit.to_string()), ("order", "asc".to_owned())];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor.to_owned()));
        }

        // A page the endpoint reports as absent is **not** an empty page. Collapsing the
        // two would turn "this collection does not exist" into "this account has no
        // history", which reads as a complete result and is the single most misleading
        // thing the network layer could do. Only a success with an empty `records` array
        // means the collection is empty.
        let page: Option<Collection<T>> = self.get(cancellation, path, &query).await?;
        let page = page.ok_or_else(|| {
            errors::classify_status(
                self.endpoint.as_str(),
                404,
                &format!(
                    "the endpoint reports no collection at {path}; an absent page is not an \
                     empty one"
                ),
            )
        })?;
        Ok(page.embedded.records)
    }
}

/// The largest page size the engine will request from a Horizon collection.
pub const MAX_COLLECTION_LIMIT: usize = 200;

/// Extracts Horizon's own error description, when the body contains one.
///
/// Horizon reports errors as `{"title": ..., "detail": ...}`. Preserving the
/// service's own words is worth more than a restatement of the status code,
/// because only the service knows why it refused.
fn horizon_error_detail(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => {
            let title = value.get("title").and_then(serde_json::Value::as_str);
            let detail = value.get("detail").and_then(serde_json::Value::as_str);
            match (title, detail) {
                (Some(title), Some(detail)) => format!("{title}: {detail}"),
                (Some(title), None) => title.to_owned(),
                (None, Some(detail)) => detail.to_owned(),
                (None, None) => truncate_for_message(body),
            }
        },
        Err(_) => truncate_for_message(body),
    }
}

/// Truncates a body so that an error message stays readable.
///
/// Bounded because a response body is attacker-adjacent input: an endpoint that
/// returned a very large body would otherwise produce an unusable error message,
/// and one that returned a body full of control characters would corrupt a
/// terminal.
fn truncate_for_message(body: &str) -> String {
    const LIMIT: usize = 300;
    let sanitised: String = body
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(LIMIT)
        .collect();
    if body.chars().count() > LIMIT {
        format!("{sanitised}...")
    } else {
        sanitised
    }
}

/// Resolves when cancellation has been requested.
async fn wait_for_cancellation(cancellation: &Cancellation) {
    while !cancellation.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::EngineConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn target(server: &MockServer) -> NetworkTarget {
        NetworkTarget::custom(
            "mock",
            "Test SDF Network ; September 2015",
            "https://rpc.example.test",
            Some(&server.uri()),
            &EngineConfig::default(),
        )
        .expect("a local endpoint is valid")
    }

    #[tokio::test]
    async fn a_transaction_is_read_from_horizon() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/transactions/{}", "ab".repeat(32))))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hash": "ab",
                "ledger": 99,
                "successful": true,
                "created_at": "2026-09-15T00:00:00Z",
                "source_account": "GABC",
                "operation_count": 2,
                "envelope_xdr": "AAAA"
            })))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let hash = "ab".repeat(32);
        let transaction = session
            .transaction(&Cancellation::new(), &hash)
            .await
            .expect("the request succeeds")
            .expect("the transaction exists");

        assert_eq!(transaction.ledger, Some(99));
        assert_eq!(transaction.operation_count, Some(2));
        assert_eq!(transaction.successful, Some(true));
    }

    #[tokio::test]
    async fn an_unknown_transaction_is_absent_rather_than_an_error() {
        // "Horizon does not have this transaction" is a definite answer, and
        // collapsing it into a transport failure would make an absence look like an
        // inability to check.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "title": "Resource Missing"
            })))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let found = session
            .transaction(&Cancellation::new(), &"ab".repeat(32))
            .await
            .expect("the endpoint answered");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn a_rate_limit_is_retried_and_the_service_reason_is_preserved() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "title": "Too Many Requests",
                "detail": "slow down"
            })))
            .expect(3)
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let error = session
            .transaction(&Cancellation::new(), &"ab".repeat(32))
            .await
            .expect_err("the endpoint never recovers");

        assert!(error.retryable());
        let rendered = error.to_string();
        assert!(rendered.contains("Too Many Requests"), "got: {rendered}");
        assert!(rendered.contains("slow down"), "got: {rendered}");
        assert!(rendered.contains("429"), "got: {rendered}");
    }

    #[tokio::test]
    async fn a_rejected_request_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "title": "Bad Request"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let error = session
            .transaction(&Cancellation::new(), &"ab".repeat(32))
            .await
            .expect_err("a rejected request fails");
        assert!(!error.retryable());
    }

    #[tokio::test]
    async fn a_malformed_body_is_reported_with_the_bytes_received() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let error = session
            .transaction(&Cancellation::new(), &"ab".repeat(32))
            .await
            .expect_err("a malformed body is a defect");

        // The bytes make the difference between a diagnosable defect and "JSON
        // error at line 1".
        assert!(
            error.to_string().contains("<html>nope</html>"),
            "got: {error}"
        );
        assert!(!error.retryable());
    }

    #[tokio::test]
    async fn a_collection_page_is_bounded_and_refuses_an_over_large_limit() {
        let server = MockServer::start().await;
        let session = HorizonSession::connect(&target(&server)).expect("connects");

        let error = session
            .collection::<HorizonLedger>(&Cancellation::new(), "ledgers", 0, None)
            .await
            .expect_err("zero is refused");
        assert!(error.to_string().contains("zero"));

        let error = session
            .collection::<HorizonLedger>(
                &Cancellation::new(),
                "ledgers",
                MAX_COLLECTION_LIMIT + 1,
                None,
            )
            .await
            .expect_err("an over-large page is refused");
        assert!(error.to_string().contains("exceeds the bound"));
    }

    #[tokio::test]
    async fn a_collection_page_is_decoded_from_its_embedded_records() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ledgers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "_embedded": {
                    "records": [
                        {"sequence": 1, "hash": "a", "closed_at": "2026-01-01T00:00:00Z"},
                        {"sequence": 2, "hash": "b"}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let ledgers: Vec<HorizonLedger> = session
            .collection(&Cancellation::new(), "ledgers", 10, None)
            .await
            .expect("the request succeeds");

        assert_eq!(ledgers.len(), 2);
        assert_eq!(ledgers[0].sequence, 1);
        assert_eq!(ledgers[1].hash.as_deref(), Some("b"));
    }

    #[test]
    fn a_long_body_is_truncated_and_control_characters_are_stripped() {
        let body = format!("{}\u{1b}[31mred", "x".repeat(1000));
        let rendered = truncate_for_message(&body);
        assert!(rendered.len() < 400);
        assert!(rendered.ends_with("..."));
        assert!(
            !rendered.contains('\u{1b}'),
            "an escape sequence from an endpoint must not reach a terminal"
        );
    }

    #[test]
    fn horizon_error_details_prefer_the_services_own_words() {
        assert_eq!(
            horizon_error_detail(r#"{"title":"Too Many Requests","detail":"slow down"}"#),
            "Too Many Requests: slow down"
        );
        assert_eq!(horizon_error_detail(r#"{"title":"Missing"}"#), "Missing");
        assert_eq!(horizon_error_detail("not json"), "not json");
    }

    #[test]
    fn a_missing_horizon_endpoint_is_an_error_rather_than_an_empty_session() {
        // A caller that asked for history would otherwise silently receive nothing.
        let config = EngineConfig::default();
        let target = NetworkTarget::custom(
            "mock",
            "Test SDF Network ; September 2015",
            "https://rpc.example.test",
            None,
            &config,
        )
        .expect("valid");
        let error = HorizonSession::connect(&target).expect_err("no Horizon endpoint");
        assert!(error.to_string().contains("no Horizon endpoint"));
    }
}
