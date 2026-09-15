//! The structured error model.
//!
//! Every failure the engine can produce is classified, because the specification
//! forbids collapsing failures into one generic error. The classification is not
//! cosmetic: a caller - the CLI, an integration test, a CI job - decides whether to
//! retry, whether to report a bounded result, or whether to fail outright based on
//! the category, and a single `Error` variant would make that decision impossible.
//!
//! Three properties are load-bearing.
//!
//! First, [`EngineError::category`] is total: every error names exactly one of the
//! categories the specification's `error.schema.json` enumerates.
//!
//! Second, [`EngineError::retryable`] is conservative. Only failures that are
//! transient by nature - a timeout, a rate limit, an unavailable endpoint - are
//! retryable. A malformed response is not retryable, because retrying it produces
//! the same malformed response and hides a real defect behind a delay.
//!
//! Third, and most importantly, a network failure is never represented as an empty
//! result. `amasario-network` returns an error for a failed request, and the
//! dependency resolver records the error as truncation rather than returning a
//! shorter dependency list. That distinction is the difference between "no
//! dependency was found" and "the search did not complete", and a consumer cannot
//! recover it after the fact.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The failure categories enumerated by the specification's error schema.
///
/// The variants correspond one-to-one with the `category` enumeration in
/// `schema/error.schema.json`. Adding a variant here without adding it there, or
/// the reverse, is caught by `crates/amasario-core/tests/error_categories.rs`,
/// which reads the published schema and compares it against [`ErrorCategory::all`].
///
/// That test is ignored by default because it needs the specification repository
/// checked out alongside this one, and a test suite that fails without an unrelated
/// checkout is one people stop running. CI checks the specification out and runs it
/// explicitly:
///
/// ```console
/// AMASARIO_SPEC_DIR=../amasario-provenance-spec cargo test -p amasario-core -- --include-ignored
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ErrorCategory {
    /// The engine's own configuration was invalid or inconsistent.
    Configuration,
    /// A request to a network endpoint failed, timed out or was rate limited.
    Network,
    /// A contract could not be inspected: absent, malformed, or not a contract.
    Contract,
    /// A provenance claim could not be evaluated, or was contradicted.
    Provenance,
    /// A dependency could not be established on the evidence available.
    Dependency,
    /// A graph could not be constructed or traversed as requested.
    Graph,
    /// An impact analysis could not be performed as requested.
    Impact,
    /// A snapshot could not be captured, read, or compared.
    Snapshot,
    /// A report could not be generated.
    Report,
    /// The input was produced by an incompatible specification or engine version.
    SpecificationCompatibility,
    /// An export format could not represent the requested content.
    Export,
    /// Input failed structural validation before any analysis began.
    Validation,
    /// An internal invariant was violated. Always a defect in the engine.
    Internal,
}

impl ErrorCategory {
    /// The stable wire name, matching `schema/error.schema.json`.
    ///
    /// Spelled out rather than derived from the variant name so that renaming a
    /// variant is a deliberate wire change instead of a silent one.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "CONFIGURATION",
            Self::Network => "NETWORK",
            Self::Contract => "CONTRACT",
            Self::Provenance => "PROVENANCE",
            Self::Dependency => "DEPENDENCY",
            Self::Graph => "GRAPH",
            Self::Impact => "IMPACT",
            Self::Snapshot => "SNAPSHOT",
            Self::Report => "REPORT",
            Self::SpecificationCompatibility => "SPECIFICATION_COMPATIBILITY",
            Self::Export => "EXPORT",
            Self::Validation => "VALIDATION",
            Self::Internal => "INTERNAL",
        }
    }

    /// Every category, in the order the specification's schema lists them.
    ///
    /// Present so that the conformance test in
    /// `crates/amasario-core/tests/error_categories.rs` can compare this
    /// enumeration against `schema/error.schema.json` mechanically. A category added
    /// on one side and not the other would otherwise be discovered by a consumer
    /// whose report could not be parsed, which is a late and confusing way to find
    /// out.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Configuration,
            Self::Network,
            Self::Contract,
            Self::Provenance,
            Self::Dependency,
            Self::Graph,
            Self::Impact,
            Self::Snapshot,
            Self::Report,
            Self::SpecificationCompatibility,
            Self::Export,
            Self::Validation,
            Self::Internal,
        ]
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The engine's error type.
///
/// The variants carry the detail a caller needs to act, and nothing more. In
/// particular no variant carries a credential, an endpoint password, or a
/// private key: the specification forbids logging or transporting secrets, and a
/// type that cannot hold one cannot leak one.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The request was not well-formed: a bad depth, an unknown format, a
    /// contradictory flag combination.
    #[error("invalid configuration: {0}")]
    Configuration(String),

    /// A network request failed. `retryable` is set only for conditions that a
    /// later attempt could plausibly succeed at.
    #[error("network request to {endpoint} failed: {detail}")]
    Network {
        /// The endpoint that was contacted. Never a URL containing credentials.
        endpoint: String,
        /// What went wrong, in the terms the transport reported.
        detail: String,
        /// Whether a later attempt could plausibly succeed.
        retryable: bool,
    },

    /// A response arrived but could not be interpreted as what it claimed to be.
    ///
    /// Kept distinct from [`EngineError::Network`] because retrying a malformed
    /// response is pointless: the same bytes will arrive and the same defect will
    /// be hidden behind a delay.
    #[error("malformed response from {endpoint}: {detail}")]
    MalformedResponse {
        /// The endpoint that returned the unusable response.
        endpoint: String,
        /// What about the response could not be interpreted.
        detail: String,
    },

    /// The requested contract does not exist at the stated boundary.
    ///
    /// This is an outcome, not a failure of the engine, and it is deliberately
    /// distinguishable from a transport error.
    #[error("contract {contract_id} was not found on {network} at ledger {ledger}")]
    ContractNotFound {
        /// The address that was looked up.
        contract_id: String,
        /// The network it was looked up on.
        network: String,
        /// The ledger the lookup was bounded by.
        ledger: u32,
    },

    /// A value failed the specification's structural constraints.
    #[error("validation failed at {path}: {detail}")]
    Validation {
        /// A JSON Pointer to the offending value, or a dotted path when the
        /// input did not come from JSON.
        path: String,
        /// What constraint was violated.
        detail: String,
    },

    /// The input was produced by a version the engine cannot interpret.
    ///
    /// This is never a warning. The specification requires a consumer to refuse
    /// rather than to guess, because guessing at a changed field could invert an
    /// impact conclusion.
    #[error("incompatible specification input: {detail}")]
    SpecificationCompatibility {
        /// What was incompatible and why.
        detail: String,
        /// The version found, when it could be read.
        found: Option<String>,
        /// The version the engine supports.
        supported: String,
    },

    /// A dependency could not be established on the evidence available.
    #[error("dependency could not be established: {0}")]
    Dependency(String),

    /// A provenance claim could not be evaluated.
    #[error("provenance could not be established: {0}")]
    Provenance(String),

    /// A graph could not be built or traversed as requested.
    #[error("graph operation failed: {0}")]
    Graph(String),

    /// An impact analysis could not be performed as requested.
    #[error("impact analysis failed: {0}")]
    Impact(String),

    /// A snapshot could not be captured, stored, read or compared.
    #[error("snapshot operation failed: {0}")]
    Snapshot(String),

    /// A report could not be produced in the requested form.
    #[error("report generation failed: {0}")]
    Report(String),

    /// An export format could not represent the requested content.
    #[error("export failed: {0}")]
    Export(String),

    /// An internal invariant was violated. This is always a defect.
    #[error("internal invariant violated: {0}")]
    Internal(String),
}

impl EngineError {
    /// The category this error belongs to.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::Configuration(_) => ErrorCategory::Configuration,
            Self::Network { .. } | Self::MalformedResponse { .. } => ErrorCategory::Network,
            Self::ContractNotFound { .. } => ErrorCategory::Contract,
            Self::Validation { .. } => ErrorCategory::Validation,
            Self::SpecificationCompatibility { .. } => ErrorCategory::SpecificationCompatibility,
            Self::Dependency(_) => ErrorCategory::Dependency,
            Self::Provenance(_) => ErrorCategory::Provenance,
            Self::Graph(_) => ErrorCategory::Graph,
            Self::Impact(_) => ErrorCategory::Impact,
            Self::Snapshot(_) => ErrorCategory::Snapshot,
            Self::Report(_) => ErrorCategory::Report,
            Self::Export(_) => ErrorCategory::Export,
            Self::Internal(_) => ErrorCategory::Internal,
        }
    }

    /// Whether a later attempt at the same operation could plausibly succeed.
    ///
    /// Conservative by construction: only transport-level transient conditions and
    /// internal defects are considered, and everything else is false. A caller that
    /// retried a non-retryable error would turn a defect into a delay.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Network { retryable, .. } => *retryable,
            _ => false,
        }
    }

    /// The structured `detail` object a report may attach, when the variant
    /// carries machine-readable context.
    ///
    /// Returned as JSON rather than as prose so that a consumer can act on it. The
    /// message is already available through `Display`; duplicating it here would
    /// give two sources of truth for the same sentence.
    #[must_use]
    pub fn detail(&self) -> Option<serde_json::Value> {
        match self {
            Self::Network {
                endpoint,
                retryable,
                ..
            } => Some(serde_json::json!({
                "endpoint": endpoint,
                "retryable": retryable,
            })),
            Self::MalformedResponse { endpoint, .. } => {
                Some(serde_json::json!({ "endpoint": endpoint }))
            },
            Self::ContractNotFound {
                contract_id,
                network,
                ledger,
            } => Some(serde_json::json!({
                "contractId": contract_id,
                "network": network,
                "ledger": ledger,
            })),
            Self::Validation { path, .. } => Some(serde_json::json!({ "pointer": path })),
            Self::SpecificationCompatibility {
                found, supported, ..
            } => Some(serde_json::json!({
                "found": found,
                "supported": supported,
            })),
            _ => None,
        }
    }

    /// A stable machine-readable code, derived from the category and the variant.
    ///
    /// A code is not a substitute for the category: the category drives control
    /// flow and the code supports reporting and alerting. They are derived from the
    /// same source so they cannot disagree.
    #[must_use]
    pub fn code(&self) -> String {
        let suffix = match self {
            Self::Configuration(_) => "INVALID",
            Self::Network {
                retryable: true, ..
            } => "TRANSIENT",
            Self::Network {
                retryable: false, ..
            } => "PERMANENT",
            Self::MalformedResponse { .. } => "MALFORMED_RESPONSE",
            Self::ContractNotFound { .. } => "NOT_FOUND",
            Self::Validation { .. } => "SCHEMA_VIOLATION",
            Self::SpecificationCompatibility { .. } => "VERSION_MISMATCH",
            Self::Dependency(_) => "UNESTABLISHED",
            Self::Provenance(_) => "UNESTABLISHED",
            Self::Graph(_) => "OPERATION_FAILED",
            Self::Impact(_) => "ANALYSIS_FAILED",
            Self::Snapshot(_) => "OPERATION_FAILED",
            Self::Report(_) => "GENERATION_FAILED",
            Self::Export(_) => "FORMAT_FAILED",
            Self::Internal(_) => "INVARIANT_VIOLATED",
        };
        format!("AMASARIO_{}_{}", self.category().as_str(), suffix)
    }

    /// A transport failure that a later attempt could plausibly succeed at.
    #[must_use]
    pub fn transient_network(endpoint: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Network {
            endpoint: endpoint.into(),
            detail: detail.into(),
            retryable: true,
        }
    }

    /// A transport failure that a later attempt would repeat.
    ///
    /// Used for conditions such as an unresolvable host name or a rejected
    /// certificate, where retrying cannot help and pretending otherwise would
    /// delay the report of a real defect.
    #[must_use]
    pub fn permanent_network(endpoint: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Network {
            endpoint: endpoint.into(),
            detail: detail.into(),
            retryable: false,
        }
    }
}

/// The engine's result type.
pub type Result<T, E = EngineError> = std::result::Result<T, E>;

/// Collects several failures so that a validation pass reports all of them.
///
/// A validator that returns at the first problem forces a contributor to fix one
/// issue per run, and more importantly hides how many other issues exist. This is
/// the mechanism the specification's own tooling uses, and the engine's input
/// validation follows it for the same reason.
#[derive(Debug, Default)]
pub struct ErrorCollection {
    errors: Vec<EngineError>,
}

impl ErrorCollection {
    /// An empty collection.
    #[must_use]
    pub const fn new() -> Self {
        Self { errors: Vec::new() }
    }

    /// Records a failure.
    pub fn push(&mut self, error: EngineError) {
        self.errors.push(error);
    }

    /// Records a failure only if the condition does not hold.
    ///
    /// Returns the value so the caller can continue building the object it is
    /// validating; a validation pass that stopped at the first problem would
    /// report less than it could.
    pub fn check(&mut self, condition: bool, path: &str, detail: &str) -> bool {
        if condition {
            true
        } else {
            self.errors.push(EngineError::Validation {
                path: path.to_owned(),
                detail: detail.to_owned(),
            });
            false
        }
    }

    /// Whether any failure has been recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// How many failures have been recorded.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.errors.len()
    }

    /// The recorded failures.
    #[must_use]
    pub fn errors(&self) -> &[EngineError] {
        &self.errors
    }

    /// Consumes the collection, yielding the first failure if there is one.
    ///
    /// The first is preferred over the last because validation is reported in the
    /// order the input was read, and the earliest problem is the one a reader is
    /// most likely to fix first.
    pub fn into_result(self) -> Result<()> {
        match self.errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Consumes the collection, yielding a summary failure if it is non-empty.
    pub fn into_summary(self, context: &str) -> Result<()> {
        if self.errors.is_empty() {
            return Ok(());
        }
        let first = &self.errors[0];
        Err(EngineError::Validation {
            path: context.to_owned(),
            detail: format!("{} problem(s) found; first: {first}", self.errors.len()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_has_a_distinct_wire_name() {
        let categories = [
            ErrorCategory::Configuration,
            ErrorCategory::Network,
            ErrorCategory::Contract,
            ErrorCategory::Provenance,
            ErrorCategory::Dependency,
            ErrorCategory::Graph,
            ErrorCategory::Impact,
            ErrorCategory::Snapshot,
            ErrorCategory::Report,
            ErrorCategory::SpecificationCompatibility,
            ErrorCategory::Export,
            ErrorCategory::Validation,
            ErrorCategory::Internal,
        ];
        let mut names: Vec<&str> = categories.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "category wire names must be unique");
    }

    #[test]
    fn category_wire_names_are_screaming_snake_case() {
        let names = [
            ErrorCategory::Configuration.as_str(),
            ErrorCategory::Network.as_str(),
            ErrorCategory::Contract.as_str(),
            ErrorCategory::Provenance.as_str(),
            ErrorCategory::Dependency.as_str(),
            ErrorCategory::Graph.as_str(),
            ErrorCategory::Impact.as_str(),
            ErrorCategory::Snapshot.as_str(),
            ErrorCategory::Report.as_str(),
            ErrorCategory::SpecificationCompatibility.as_str(),
            ErrorCategory::Export.as_str(),
            ErrorCategory::Validation.as_str(),
            ErrorCategory::Internal.as_str(),
        ];
        for name in names {
            assert!(
                name.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
                "{name} is not SCREAMING_SNAKE_CASE"
            );
            assert!(
                !name.starts_with('_') && !name.ends_with('_'),
                "{name} has a stray underscore"
            );
        }
    }

    #[test]
    fn a_transient_network_failure_is_retryable_and_a_permanent_one_is_not() {
        let transient = EngineError::transient_network("https://example.test", "timed out");
        let permanent = EngineError::permanent_network("https://example.test", "bad certificate");

        assert!(transient.retryable());
        assert!(!permanent.retryable());
        assert_eq!(transient.category(), ErrorCategory::Network);
        assert_eq!(permanent.category(), ErrorCategory::Network);
        assert_eq!(transient.category(), permanent.category());
        // The codes must differ, because an operator alerting on a code needs to
        // distinguish "try again" from "stop".
        assert_ne!(transient.code(), permanent.code());
    }

    #[test]
    fn a_malformed_response_is_never_retryable() {
        // Retrying malformed bytes returns the same malformed bytes, so marking it
        // retryable would hide a defect behind a delay.
        let error = EngineError::MalformedResponse {
            endpoint: "https://example.test".to_owned(),
            detail: "missing result field".to_owned(),
        };
        assert!(!error.retryable());
        assert!(error.code().contains("MALFORMED_RESPONSE"));
    }

    #[test]
    fn a_contract_not_found_is_an_outcome_and_not_a_transport_error() {
        let error = EngineError::ContractNotFound {
            contract_id: "C2IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26XB".to_owned(),
            network: "testnet".to_owned(),
            ledger: 1234567,
        };
        assert_eq!(error.category(), ErrorCategory::Contract);
        // Distinguishing this from a network error is the point: a caller must be
        // able to tell "the contract is absent" from "the query failed".
        assert_ne!(error.category(), ErrorCategory::Network);
    }

    #[test]
    fn the_code_is_derived_from_the_category_so_the_two_cannot_disagree() {
        let cases: Vec<EngineError> = vec![
            EngineError::Configuration("bad depth".to_owned()),
            EngineError::Graph("cycle".to_owned()),
            EngineError::Internal("unreachable".to_owned()),
        ];
        for error in cases {
            let code = error.code();
            assert!(
                code.starts_with("AMASARIO_"),
                "code {code} must be namespaced"
            );
            assert!(
                code.contains(error.category().as_str()),
                "code {code} must contain its category {}",
                error.category()
            );
        }
    }

    #[test]
    fn detail_is_only_present_when_the_variant_carries_machine_readable_context() {
        let with_detail = EngineError::ContractNotFound {
            contract_id: "C2IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26XB".to_owned(),
            network: "testnet".to_owned(),
            ledger: 12,
        };
        assert!(with_detail.detail().is_some());
        assert!(EngineError::Internal("boom".to_owned()).detail().is_none());
    }

    #[test]
    fn a_validation_pass_reports_every_problem_rather_than_the_first() {
        let mut collection = ErrorCollection::new();
        assert!(collection.is_empty());

        collection.check(true, "/a", "should not be recorded");
        assert!(collection.is_empty());

        collection.check(false, "/b", "first problem");
        collection.check(false, "/c", "second problem");
        assert_eq!(collection.len(), 2);
        assert!(!collection.is_empty());
        assert_eq!(collection.errors().len(), 2);

        let error = collection
            .into_result()
            .expect_err("two problems were recorded");
        assert_eq!(error.category(), ErrorCategory::Validation);
        // The first problem is reported, because validation follows input order and
        // the earliest problem is the one a reader fixes first.
        assert!(error.to_string().contains("first problem"));
    }

    #[test]
    fn an_empty_collection_succeeds() {
        assert!(ErrorCollection::new().into_result().is_ok());
        assert!(ErrorCollection::new().into_summary("/document").is_ok());
    }

    #[test]
    fn the_summary_counts_the_problems_it_found() {
        let mut collection = ErrorCollection::new();
        collection.check(false, "/a", "one");
        collection.check(false, "/b", "two");
        collection.check(false, "/c", "three");
        let error = collection
            .into_summary("/document")
            .expect_err("three problems were recorded");
        assert!(error.to_string().contains("3 problem(s)"));
    }

    #[test]
    fn an_error_displayed_message_carries_the_context_a_reader_needs() {
        let error = EngineError::Network {
            endpoint: "https://soroban-testnet.stellar.org".to_owned(),
            detail: "connection reset".to_owned(),
            retryable: true,
        };
        let rendered = error.to_string();
        assert!(rendered.contains("https://soroban-testnet.stellar.org"));
        assert!(rendered.contains("connection reset"));
    }
}
