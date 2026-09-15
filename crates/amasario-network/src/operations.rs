//! Operation records from Horizon.
//!
//! An operation is one effect of a transaction. The distinction that matters here
//! is between an operation's *common* fields, which Horizon reports for every kind
//! and which the engine models, and its *kind-specific* fields, which it does not.
//!
//! Inventing a model for every operation type would mean claiming to understand
//! fields the engine does not use; discarding them would mean throwing away
//! evidence a future consumer needs. The type-specific detail is therefore kept
//! verbatim as JSON, which is honest - it is exactly what the endpoint said - and
//! is enough for the engine's dependent crates to read a specific field when a rule
//! calls for it.
//!
//! # Ordering
//!
//! Operations are returned in Horizon's order, which is ascending by ledger and
//! then by application order. They are **not** re-sorted, because Horizon's order
//! is the chain's order - the order the effects actually occurred in - and
//! re-sorting by identifier would destroy it.

use serde::Deserialize;

use amasario_core::{Cancellation, EngineError, Result, TransactionHash};

use crate::horizon::HorizonSession;

/// An operation record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OperationRecord {
    /// Horizon's identifier for the operation.
    #[serde(default)]
    pub id: Option<String>,
    /// The operation's type name, such as `invoke_host_function`.
    ///
    /// Renamed because Horizon calls this field `type`, which is a Rust keyword;
    /// without the rename the field would silently deserialise to `None` and every
    /// operation would appear to be of an unknown kind.
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    /// The operation's numeric type.
    #[serde(default, rename = "type_i")]
    pub type_index: Option<i32>,
    /// The hash of the transaction the operation belongs to.
    #[serde(default)]
    pub transaction_hash: Option<String>,
    /// When the operation was applied, as an RFC 3339 timestamp.
    #[serde(default)]
    pub created_at: Option<String>,
    /// The account the operation's source is, when it differs from the
    /// transaction's source.
    #[serde(default)]
    pub source_account: Option<String>,
    /// The operation's own fields, verbatim.
    ///
    /// Kept as-is rather than modelled: see the module documentation.
    #[serde(flatten)]
    pub detail: serde_json::Map<String, serde_json::Value>,
}

impl OperationRecord {
    /// Reads a field from the operation's type-specific detail.
    ///
    /// Returns `None` when the field is absent, which is the honest answer for an
    /// operation of a different kind rather than a default that a caller could
    /// mistake for a fact.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&serde_json::Value> {
        self.detail.get(name)
    }

    /// A field as a string, when it is one.
    #[must_use]
    pub fn string_field(&self, name: &str) -> Option<&str> {
        self.field(name).and_then(serde_json::Value::as_str)
    }

    /// Whether this operation is a contract invocation.
    #[must_use]
    pub fn is_invoke_host_function(&self) -> bool {
        self.type_name.as_deref() == Some("invoke_host_function")
    }

    /// The contract an `invoke_host_function` operation targets, when Horizon
    /// reported one.
    #[must_use]
    pub fn contract_id(&self) -> Option<&str> {
        self.string_field("contract_id")
    }
}

/// Reads the operations of a transaction.
///
/// # Errors
///
/// Returns [`EngineError::Validation`] when the limit is zero, and a classified
/// error when the request fails.
pub async fn operations_for_transaction(
    horizon: &HorizonSession,
    cancellation: &Cancellation,
    hash: &TransactionHash,
    limit: usize,
) -> Result<Vec<OperationRecord>> {
    let path = format!("transactions/{}/operations", hash.as_str());
    horizon.collection(cancellation, &path, limit, None).await
}

/// Reads one page of an account's operations.
///
/// # Errors
///
/// Returns [`EngineError::Configuration`] for an empty account or a zero limit,
/// and a classified error when the request fails.
pub async fn operations_for_account(
    horizon: &HorizonSession,
    cancellation: &Cancellation,
    account: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<Vec<OperationRecord>> {
    if account.is_empty() {
        return Err(EngineError::Configuration(
            "an operations lookup needs an account".to_owned(),
        ));
    }
    let path = format!("accounts/{account}/operations");
    horizon.collection(cancellation, &path, limit, cursor).await
}

/// Reads one page of a ledger's operations.
///
/// # Errors
///
/// Returns [`EngineError::Configuration`] for a zero limit, and a classified error
/// when the request fails.
pub async fn operations_for_ledger(
    horizon: &HorizonSession,
    cancellation: &Cancellation,
    sequence: u32,
    limit: usize,
) -> Result<Vec<OperationRecord>> {
    let path = format!("ledgers/{sequence}/operations");
    horizon.collection(cancellation, &path, limit, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::NetworkTarget;
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

    fn operations_body() -> serde_json::Value {
        serde_json::json!({
            "_embedded": {
                "records": [
                    {
                        "id": "1",
                        "type": "invoke_host_function",
                        "type_i": 24,
                        "transaction_hash": "ab",
                        "created_at": "2026-09-15T00:00:00Z",
                        "contract_id": "CAAAA",
                        "function": "transfer"
                    },
                    {
                        "id": "2",
                        "type": "payment",
                        "type_i": 1,
                        "transaction_hash": "ab"
                    }
                ]
            }
        })
    }

    #[tokio::test]
    async fn a_transactions_operations_are_read_in_the_chains_order() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/transactions/{}/operations",
                "ab".repeat(32)
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(operations_body()))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let hash = TransactionHash::new("ab".repeat(32)).expect("a valid hash");
        let operations = operations_for_transaction(&session, &Cancellation::new(), &hash, 10)
            .await
            .expect("the request succeeds");

        assert_eq!(operations.len(), 2);
        // Horizon's order is the chain's order; re-sorting by identifier would
        // destroy it.
        assert_eq!(operations[0].id.as_deref(), Some("1"));
        assert_eq!(operations[1].id.as_deref(), Some("2"));
    }

    #[tokio::test]
    async fn only_an_invocation_is_recognised_as_one() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(operations_body()))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let hash = TransactionHash::new("ab".repeat(32)).expect("a valid hash");
        let operations = operations_for_transaction(&session, &Cancellation::new(), &hash, 10)
            .await
            .expect("the request succeeds");

        assert!(operations[0].is_invoke_host_function());
        assert_eq!(operations[0].contract_id(), Some("CAAAA"));
        // A payment is not an invocation, so it must not be reported as one.
        assert!(!operations[1].is_invoke_host_function());
        assert_eq!(operations[1].contract_id(), None);
    }

    #[tokio::test]
    async fn a_field_an_operation_does_not_have_is_absent_rather_than_defaulted() {
        // Returning a default would let a caller mistake "this operation has no such
        // field" for "the field was empty".
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(operations_body()))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let hash = TransactionHash::new("ab".repeat(32)).expect("a valid hash");
        let operations = operations_for_transaction(&session, &Cancellation::new(), &hash, 10)
            .await
            .expect("the request succeeds");

        assert_eq!(operations[1].field("contract_id"), None);
        assert_eq!(operations[0].string_field("function"), Some("transfer"));
    }

    #[tokio::test]
    async fn an_accounts_operations_are_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/accounts/GABC/operations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(operations_body()))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let operations = operations_for_account(&session, &Cancellation::new(), "GABC", 5, None)
            .await
            .expect("the request succeeds");
        assert_eq!(operations.len(), 2);
    }

    #[tokio::test]
    async fn an_empty_account_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let error = operations_for_account(&session, &Cancellation::new(), "", 5, None)
            .await
            .expect_err("an empty account is refused");
        assert!(error.to_string().contains("needs an account"));
    }

    #[tokio::test]
    async fn a_ledgers_operations_are_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ledgers/12/operations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(operations_body()))
            .mount(&server)
            .await;

        let session = HorizonSession::connect(&target(&server)).expect("connects");
        let operations = operations_for_ledger(&session, &Cancellation::new(), 12, 5)
            .await
            .expect("the request succeeds");
        assert_eq!(operations.len(), 2);
    }
}
