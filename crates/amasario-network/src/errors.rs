//! Classification of transport failures into the engine's error model.
//!
//! A provenance engine's most damaging output is a wrong `VERIFIED`, and the
//! second most damaging is a *short* result that reads as a complete one. Both
//! come from the same mistake: treating a failure as an absence. This module
//! exists so that every way a network request can fail becomes a distinct,
//! classified error that a caller can act on, and never an empty list.
//!
//! # What is classified, and how
//!
//! [`classify`] maps the Stellar RPC client's error type into
//! [`EngineError`]. The classification is not cosmetic: [`EngineError::retryable`]
//! decides whether the retry layer makes another attempt, and
//! [`EngineError::category`] decides whether a caller reports a bounded result or
//! fails outright.
//!
//! The rule for retryability is stated once, in [`classify_code`]: a failure that
//! says **the request was rejected** is not retryable, because sending the same
//! rejected request again produces the same rejection; a failure that says **the
//! server had a problem** is retryable, because the request itself was not the
//! complaint.
//!
//! # What is deliberately not classified
//!
//! The `-32000` to `-32099` range is reserved by the JSON-RPC 2.0 specification
//! for implementation-defined server errors. The engine does **not** assign
//! invented meanings to individual codes in that range. Doing so would mean
//! guessing at Stellar RPC semantics, which is exactly what the specification
//! forbids: a guess that turned "not found" into "transient" would make the engine
//! return an empty result where it should have reported a bounded one. The raw
//! code and message are preserved in the error detail so that an operator can act
//! on the real meaning without the engine having to invent it.
//!
//! One consequence of that policy is handled structurally rather than by matching
//! an error: the documented behaviour of `getEvents` and `getLedgers` is that they
//! fail when `startLedger` falls outside the range the node retains
//! (<https://developers.stellar.org/docs/data/apis/rpc/api-reference/methods/getEvents>).
//! Rather than recognise that failure after the fact, [`crate::events`] bounds its
//! scan against the node's reported oldest ledger, so the request is never made
//! outside the retention window in the first place.

use amasario_core::EngineError;
use jsonrpsee_core::ClientError;
use jsonrpsee_core::http_helpers::HttpError;
use stellar_rpc_client::Error as RpcError;

/// The JSON-RPC 2.0 error codes, which are the only codes the engine assigns
/// meaning to.
///
/// Taken from the JSON-RPC 2.0 specification's reserved ranges. No code outside
/// these is interpreted.
pub mod codes {
    /// The request could not be parsed as JSON.
    pub const PARSE_ERROR: i32 = -32700;
    /// The request was not a valid JSON-RPC request object.
    pub const INVALID_REQUEST: i32 = -32600;
    /// The requested method is not implemented by this endpoint.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// The request parameters were rejected.
    pub const INVALID_PARAMS: i32 = -32602;
    /// The JSON-RPC layer itself failed on the server.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// The lowest code reserved for implementation-defined server errors.
    pub const SERVER_ERROR_MIN: i32 = -32099;
    /// The highest code reserved for implementation-defined server errors.
    pub const SERVER_ERROR_MAX: i32 = -32000;
}

/// Classifies a failure reported by the Stellar RPC client.
///
/// `endpoint` is recorded on the error so that a message names the source of the
/// observation; it must never contain credentials, which is why the engine takes
/// the endpoint from [`crate::client::RpcEndpoint`], whose constructor rejects a
/// URL carrying user information.
#[must_use]
pub fn classify(endpoint: &str, error: &RpcError) -> EngineError {
    match error {
        RpcError::JsonRpc(inner) => classify_client(endpoint, inner),

        // A response arrived but was not what it claimed to be. Retrying would
        // deliver the same bytes, so the engine reports a defect rather than
        // delaying it behind a retry.
        RpcError::Serde(_) | RpcError::Xdr(_) | RpcError::InvalidResponse => {
            EngineError::MalformedResponse {
                endpoint: endpoint.to_owned(),
                detail: error.to_string(),
            }
        },
        RpcError::MissingResult | RpcError::MissingError => EngineError::MalformedResponse {
            endpoint: endpoint.to_owned(),
            detail: error.to_string(),
        },

        // The endpoint answered, and it serves a different chain than the one
        // requested. That is not a transient condition and retrying cannot fix it,
        // so it is reported as a permanent failure with the mismatch named.
        RpcError::InvalidNetworkPassphrase { .. } => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail: format!(
                "{error}; the endpoint serves a different Stellar network than the one requested"
            ),
            retryable: false,
        },

        // A malformed endpoint is a configuration problem, not a transport one:
        // the engine never reached the network.
        RpcError::InvalidRpcUrl(_)
        | RpcError::InvalidRpcUrlFromUriParts(_)
        | RpcError::InvalidUrl(_) => EngineError::Configuration(format!("{error}")),

        // Reached only if a caller passes an address this layer already validated.
        RpcError::InvalidAddress(_) => EngineError::Validation {
            path: "/contract".to_owned(),
            detail: error.to_string(),
        },

        // The engine deliberately does not rely on the client's own absent-resource
        // error: it looks up ledger entries directly, and an absent entry is an
        // empty result that [`crate::contracts`] turns into a `ContractNotFound`
        // carrying the boundary it actually observed. This arm is a defensive
        // fallback so that the variant cannot be silently ignored.
        RpcError::NotFound(kind, id) => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail: format!("the endpoint reported {kind} {id} as absent"),
            retryable: false,
        },

        // A cursor the engine itself constructed was rejected. That is a defect on
        // this side of the wire, not a network condition.
        RpcError::InvalidCursor => EngineError::Validation {
            path: "/cursor".to_owned(),
            detail: "the endpoint rejected a pagination cursor as invalid".to_owned(),
        },

        // The remaining variants describe transaction submission and simulation,
        // which the analysis pipeline never performs: Amasario observes the network
        // and does not mutate it. They are classified as permanent failures so that
        // an unexpected occurrence is reported rather than mistaken for a transient
        // condition, and so that adding a variant upstream fails loudly here rather
        // than being silently treated as success.
        _ => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail: format!("unclassified RPC failure: {error}"),
            retryable: false,
        },
    }
}

/// Classifies a failure from the JSON-RPC transport layer.
fn classify_client(endpoint: &str, error: &ClientError) -> EngineError {
    match error {
        // A real answer from the server: the server declined the request.
        ClientError::Call(object) => classify_code(endpoint, object.code(), object.message()),

        // A transport failure is *not* automatically transient. The transport
        // reports a response body it could not read as a transport failure, and
        // retrying that returns the same bytes; the real cause has to be recovered
        // from the error chain. See `classify_transport`.
        ClientError::Transport(inner) => classify_transport(endpoint, inner.as_ref(), error),

        // The socket died, the client needs restarting, or the request timed out.
        // Each is a condition a later attempt can plausibly clear.
        ClientError::RestartNeeded(_)
        | ClientError::RequestTimeout
        | ClientError::ServiceDisconnect => {
            EngineError::transient_network(endpoint, error.to_string())
        },

        // The response arrived but could not be decoded.
        ClientError::ParseError(_) => EngineError::MalformedResponse {
            endpoint: endpoint.to_owned(),
            detail: error.to_string(),
        },

        // Protocol-level misuse by the caller: a request the engine built was
        // structurally invalid, or used a feature this transport does not have.
        ClientError::InvalidSubscriptionId
        | ClientError::InvalidRequestId(_)
        | ClientError::EmptyBatchRequest(_)
        | ClientError::RegisterMethod(_)
        | ClientError::HttpNotImplemented
        | ClientError::Custom(_) => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail: format!("the request was rejected: {error}"),
            retryable: false,
        },
    }
}

/// Classifies a transport failure by recovering the HTTP-body cause when there is
/// one.
///
/// The JSON-RPC transport reports "I could not read this response body as JSON"
/// through the same variant as "the connection failed", which would make a
/// malformed response look transient and be retried. Retrying a malformed response
/// returns the same malformed response and hides a real defect behind a delay, so
/// the distinction is recovered by walking the error chain to the transport's own
/// HTTP-body error.
///
/// This is also the case an operator is most likely to meet in practice: an
/// endpoint or an intermediary that answers a JSON-RPC request with an HTML error
/// page.
fn classify_transport(
    endpoint: &str,
    inner: &(dyn std::error::Error + 'static),
    error: &ClientError,
) -> EngineError {
    match http_body_failure(inner) {
        Some(HttpError::Malformed) => EngineError::MalformedResponse {
            endpoint: endpoint.to_owned(),
            detail: format!(
                "{error}; the response could not be read as JSON-RPC. A retry would receive the \
                 same bytes, so this is reported as a defect rather than retried."
            ),
        },
        // The message exceeded a size limit. Retrying sends the same request and
        // provokes the same refusal.
        Some(HttpError::TooLarge) => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail: format!(
                "{error}; narrow the request or raise the transport's response size limit"
            ),
            retryable: false,
        },
        // The chain did not expose the body failure, but the transport's own
        // message for a body it could not read did. This fallback exists because
        // the transport wraps that failure in a type that is not publicly nameable
        // and declares its `source` as transparent, which hides the cause from a
        // downcast on the real request path.
        //
        // The expected wording is produced by `HttpError`'s own `Display` rather
        // than transcribed from it, so if the library rewords the message the check
        // stops matching and the failure degrades to a bounded retry. Degrading to
        // a retry is the safe direction: it costs two extra requests, where a wrong
        // "malformed" verdict would hide a transient failure from the retry layer.
        None if error.to_string() == HttpError::Malformed.to_string() => {
            EngineError::MalformedResponse {
                endpoint: endpoint.to_owned(),
                detail: format!(
                    "{error}; the response could not be read as JSON-RPC. A retry would receive \
                     the same bytes, so this is reported as a defect rather than retried."
                ),
            }
        },

        // A broken stream, or a body failure the chain does not identify. The
        // request was not rejected, so a later attempt can plausibly succeed.
        Some(HttpError::Stream(_)) | None => {
            EngineError::transient_network(endpoint, error.to_string())
        },
    }
}

/// The transport's own HTTP-body failure, when the error chain contains one.
///
/// Walks the chain rather than downcasting the top-level error because the
/// transport wraps its body failure in an intermediate type that is not publicly
/// nameable, so the cause is only reachable through `source`.
fn http_body_failure<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a HttpError> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = current {
        if let Some(http) = cause.downcast_ref::<HttpError>() {
            return Some(http);
        }
        current = cause.source();
    }
    None
}

/// Classifies a JSON-RPC error code.
///
/// The split is by *what the failure implicates*, not by severity:
///
/// * the request-shaped codes ([`codes::PARSE_ERROR`], [`codes::INVALID_REQUEST`],
///   [`codes::METHOD_NOT_FOUND`], [`codes::INVALID_PARAMS`]) mean the endpoint
///   rejected what was sent. Repeating it produces the same rejection, so they are
///   not retryable.
/// * [`codes::INTERNAL_ERROR`] and the implementation-defined server range mean
///   the request was not the complaint. A later attempt can plausibly succeed, so
///   they are retryable - bounded, as always, by the configured attempt limit.
#[must_use]
pub fn classify_code(endpoint: &str, code: i32, message: &str) -> EngineError {
    let detail = format!("JSON-RPC error {code}: {message}");

    match code {
        codes::METHOD_NOT_FOUND => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail: format!(
                "{detail}; the endpoint does not implement this method, which usually means it \
                 predates it. Raise the endpoint's version or use one that supports the method."
            ),
            retryable: false,
        },
        codes::PARSE_ERROR | codes::INVALID_REQUEST | codes::INVALID_PARAMS => {
            EngineError::Network {
                endpoint: endpoint.to_owned(),
                detail,
                retryable: false,
            }
        },
        codes::INTERNAL_ERROR => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail,
            retryable: true,
        },
        code if (codes::SERVER_ERROR_MIN..=codes::SERVER_ERROR_MAX).contains(&code) => {
            EngineError::Network {
                endpoint: endpoint.to_owned(),
                detail,
                retryable: true,
            }
        },
        _ => EngineError::Network {
            endpoint: endpoint.to_owned(),
            detail,
            retryable: false,
        },
    }
}

/// Whether an HTTP status means the requested resource is absent rather than that
/// the request failed.
///
/// Callers use this to return an explicit absence, which is a different statement
/// from a failure. Collapsing the two is what turns "the transaction is not on this
/// network" into "the network could not be reached".
#[must_use]
pub const fn status_is_absent(status: u16) -> bool {
    status == 404
}

/// Whether an HTTP status means the request should be attempted again.
///
/// A rate limit and a server-side error are transient; a rejected request is not.
/// `429` is called out separately because it is the condition a public endpoint is
/// most likely to impose on an analysis tool, and the one most easily mistaken for
/// a complete result.
#[must_use]
pub const fn status_is_transient(status: u16) -> bool {
    status == 429 || status == 408 || status >= 500
}

/// Classifies a non-success HTTP response from a REST endpoint.
///
/// `detail` should carry the endpoint's own message where one was readable, since
/// an operator diagnosing a failure needs the server's reason rather than a
/// restatement of the status code.
#[must_use]
pub fn classify_status(endpoint: &str, status: u16, detail: &str) -> EngineError {
    let detail = format!("HTTP {status}: {detail}");
    if status_is_transient(status) {
        EngineError::transient_network(endpoint, detail)
    } else {
        EngineError::permanent_network(endpoint, detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::ErrorCategory;

    const ENDPOINT: &str = "https://soroban-testnet.stellar.org";

    #[test]
    fn a_server_side_failure_is_retryable_and_a_rejected_request_is_not() {
        // The rule this module exists to apply, asserted directly.
        for code in [codes::INTERNAL_ERROR, -32000, -32001, -32099] {
            let error = classify_code(ENDPOINT, code, "server had a problem");
            assert!(
                error.retryable(),
                "code {code} is a server-side condition and should be retried"
            );
        }

        for code in [
            codes::PARSE_ERROR,
            codes::INVALID_REQUEST,
            codes::METHOD_NOT_FOUND,
            codes::INVALID_PARAMS,
        ] {
            let error = classify_code(ENDPOINT, code, "the request was rejected");
            assert!(
                !error.retryable(),
                "code {code} rejects the request itself and must not be retried"
            );
        }
    }

    #[test]
    fn every_classified_failure_keeps_the_raw_code_and_message() {
        // An operator acting on the real meaning needs the code and the server's
        // own words; replacing them with an invented category would destroy the
        // only accurate information available.
        let error = classify_code(ENDPOINT, -32001, "startLedger too old");
        let rendered = error.to_string();
        assert!(rendered.contains("-32001"), "got: {rendered}");
        assert!(rendered.contains("startLedger too old"), "got: {rendered}");
        assert!(rendered.contains(ENDPOINT), "got: {rendered}");
    }

    #[test]
    fn a_method_the_endpoint_does_not_implement_says_so() {
        let error = classify_code(ENDPOINT, codes::METHOD_NOT_FOUND, "method not found");
        assert!(!error.retryable());
        assert!(
            error.to_string().contains("does not implement"),
            "the message must explain the likely cause: {error}"
        );
    }

    #[test]
    fn a_malformed_response_is_never_retryable() {
        let error = classify(
            ENDPOINT,
            &RpcError::Serde(serde_json::from_str::<u8>("not json").unwrap_err()),
        );
        assert_eq!(error.category(), ErrorCategory::Network);
        assert!(
            !error.retryable(),
            "retrying malformed bytes returns the same malformed bytes"
        );
        assert!(matches!(error, EngineError::MalformedResponse { .. }));
    }

    #[test]
    fn a_transport_failure_is_retryable() {
        let error = classify(ENDPOINT, &RpcError::JsonRpc(ClientError::RequestTimeout));
        assert!(error.retryable());
        assert_eq!(error.category(), ErrorCategory::Network);
    }

    #[test]
    fn a_response_body_that_is_not_json_is_a_defect_rather_than_a_transient_failure() {
        // The transport reports an unreadable body through the same variant as a
        // broken connection, so this is the case where a blanket rule would retry a
        // defect. It is the situation an operator meets whenever an endpoint or an
        // intermediary answers a JSON-RPC request with an HTML error page.
        let error = classify(
            ENDPOINT,
            &RpcError::JsonRpc(ClientError::Transport(Box::new(HttpError::Malformed))),
        );
        assert!(!error.retryable());
        assert!(matches!(error, EngineError::MalformedResponse { .. }));
        assert!(
            error.to_string().contains("could not be read as JSON-RPC"),
            "got: {error}"
        );

        // The fallback path, exercised through the real transport: the client
        // reports an unreadable body as a transport failure whose `source` is
        // transparent, so the downcast above cannot see the cause. This asserts the
        // classification still lands correctly on the path the engine actually uses.
        let error = classify(
            ENDPOINT,
            &RpcError::JsonRpc(ClientError::Transport(Box::new(std::io::Error::other(
                HttpError::Malformed.to_string(),
            )))),
        );
        assert!(!error.retryable(), "got: {error}");
        assert!(matches!(error, EngineError::MalformedResponse { .. }));
    }

    #[test]
    fn a_response_that_exceeds_the_size_limit_is_reported_as_permanent() {
        // Retrying sends the same request and provokes the same refusal.
        let error = classify(
            ENDPOINT,
            &RpcError::JsonRpc(ClientError::Transport(Box::new(HttpError::TooLarge))),
        );
        assert!(!error.retryable());
        assert!(error.to_string().contains("size limit"), "got: {error}");
    }

    #[test]
    fn a_broken_stream_is_still_retryable() {
        // The request was not rejected, so a later attempt can plausibly succeed.
        let stream = HttpError::Stream(Box::new(std::io::Error::other("connection reset")));
        let error = classify(
            ENDPOINT,
            &RpcError::JsonRpc(ClientError::Transport(Box::new(stream))),
        );
        assert!(error.retryable());
    }

    #[test]
    fn an_unidentified_body_failure_is_treated_as_transient_rather_than_permanent() {
        // The conservative direction: an unrecognised transport failure is retried
        // rather than failed, because the request was not rejected.
        let error = classify(
            ENDPOINT,
            &RpcError::JsonRpc(ClientError::Transport(Box::new(std::io::Error::other(
                "unknown",
            )))),
        );
        assert!(error.retryable());
    }

    #[test]
    fn a_disconnected_service_is_retryable() {
        let error = classify(ENDPOINT, &RpcError::JsonRpc(ClientError::ServiceDisconnect));
        assert!(error.retryable());
    }

    #[test]
    fn a_network_passphrase_mismatch_is_permanent_and_explains_itself() {
        let error = classify(
            ENDPOINT,
            &RpcError::InvalidNetworkPassphrase {
                expected: "Test SDF Network ; September 2015".to_owned(),
                server: "Public Global Stellar Network ; September 2015".to_owned(),
            },
        );
        assert_eq!(error.category(), ErrorCategory::Network);
        assert!(
            !error.retryable(),
            "the endpoint serves a different chain; retrying cannot change that"
        );
        assert!(
            error.to_string().contains("different Stellar network"),
            "got: {error}"
        );
    }

    #[test]
    fn a_malformed_endpoint_is_a_configuration_failure_not_a_transport_one() {
        // The engine never reached the network, so reporting a transport failure
        // would send an operator looking at the wrong thing.
        let error = classify(ENDPOINT, &RpcError::InvalidUrl("not a url".to_owned()));
        assert_eq!(error.category(), ErrorCategory::Configuration);
    }

    #[test]
    fn a_rejected_cursor_is_reported_as_a_defect_on_this_side() {
        let error = classify(ENDPOINT, &RpcError::InvalidCursor);
        assert_eq!(error.category(), ErrorCategory::Validation);
    }

    #[test]
    fn an_unclassified_variant_fails_rather_than_appearing_to_succeed() {
        // The catch-all exists so that an upstream addition cannot be silently
        // treated as success. It must be a failure, and a permanent one.
        let error = classify(ENDPOINT, &RpcError::MissingOp);
        assert_eq!(error.category(), ErrorCategory::Network);
        assert!(!error.retryable());
        assert!(error.to_string().contains("unclassified"));
    }

    #[test]
    fn only_the_documented_transient_statuses_are_retried() {
        assert!(status_is_transient(429));
        assert!(status_is_transient(408));
        assert!(status_is_transient(500));
        assert!(status_is_transient(503));

        // These are definite answers, so retrying wastes time and delays a real
        // failure becoming visible.
        assert!(!status_is_transient(400));
        assert!(!status_is_transient(403));
        assert!(!status_is_transient(404));
    }

    #[test]
    fn a_rate_limit_is_retried_and_a_bad_request_is_not() {
        let limited = classify_status(ENDPOINT, 429, "too many requests");
        let rejected = classify_status(ENDPOINT, 400, "bad request");
        assert!(limited.retryable());
        assert!(!rejected.retryable());
        // Both are network failures, but only one is worth another attempt, which is
        // precisely the distinction the retry layer needs.
        assert_eq!(limited.category(), rejected.category());
    }

    #[test]
    fn absence_is_distinguishable_from_failure() {
        assert!(status_is_absent(404));
        // A 404 is a definite answer and must not be treated as a transient
        // condition, or an absence would be reported as an incomplete search.
        assert!(!status_is_transient(404));
        assert!(!status_is_absent(500));
    }
}
