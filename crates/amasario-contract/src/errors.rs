//! The failure modes contract inspection can report, and how each is classified.
//!
//! This module exists because the interesting question in inspection is not "did
//! it work" but *what kind of thing went wrong*, and the answer is not uniform.
//! A contract that does not exist is a finding; a contract whose instance entry
//! names a WASM module whose code entry is absent is a provenance anomaly; a
//! response that is not a contract-data entry at all is a defect in the request or
//! the endpoint. Collapsing those three into one failure would make the engine
//! unable to tell a consumer which of them happened.
//!
//! [`InspectionFailure`] enumerates them and [`InspectionFailure::into_error`]
//! converts one into the engine's structured error, carrying the identifiers that
//! make it actionable. The mapping is asserted by a test rather than left to the
//! reader, because the category drives whether the CLI fails the run, reports a
//! partial result, or reports a finding.

use amasario_core::{EngineError, ErrorCategory, LedgerSequence, Result};

/// A way contract inspection can fail.
///
/// Not `#[non_exhaustive]`: this crate is the only producer, and a consumer
/// matching on it exhaustively is the intended use - the point of the enumeration
/// is that a caller handles every case rather than a catch-all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectionFailure {
    /// No contract exists at the address, at the observed boundary.
    NotDeployed {
        /// The address that was looked up.
        contract_id: String,
        /// The network it was looked up on.
        network: String,
        /// The ledger the lookup was bounded by.
        ledger: LedgerSequence,
    },
    /// The contract exists but its instance entry names a WASM hash whose code
    /// entry was not present at this boundary.
    ///
    /// This is an anomaly rather than a routine absence: a contract whose
    /// executable was pruned, or whose code entry was never written, cannot be
    /// analysed and the reason must not be reported as "not found".
    CodeEntryAbsent {
        /// The address whose executable is missing.
        contract_id: String,
        /// The hash the instance named.
        wasm_hash: String,
    },
    /// The contract exists but its instance entry's executable is not a WASM
    /// module and not a Stellar Asset Contract, so the engine cannot say what it
    /// runs.
    UnrecognisedExecutable {
        /// The address whose executable could not be classified.
        contract_id: String,
    },
    /// The deployed module's bytes do not hash to the digest the instance entry
    /// claims.
    ///
    /// Reported as a contradiction rather than as a verification failure, because
    /// the two differ in what a consumer should do: a contradiction means the
    /// engine holds two facts that cannot both be true.
    DigestContradiction {
        /// The address whose module disagrees with its recorded hash.
        contract_id: String,
        /// The hash the network recorded.
        recorded: String,
        /// The hash the returned bytes actually produce.
        computed: String,
    },
    /// A retrieved value was not the thing its request named.
    UnexpectedEntry {
        /// What was expected, in terms a reader can act on.
        expected: String,
        /// What arrived instead.
        received: String,
    },
    /// The module's bytes are not a well-formed WebAssembly module.
    MalformedModule {
        /// What about the module was not interpretable.
        detail: String,
    },
    /// The module's contract-specification section could not be decoded.
    ///
    /// Distinct from [`InspectionFailure::MalformedModule`]: the module is a valid
    /// module and can still be verified by digest, but its declared interface is
    /// unreadable. Reporting that as a malformed module would overstate the problem
    /// and would deny the caller the digest verification that still holds.
    SpecSectionUndecodable {
        /// What about the section could not be decoded, and how far decoding got.
        detail: String,
    },
}

impl InspectionFailure {
    /// The category this failure belongs to.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::NotDeployed { .. } => ErrorCategory::Contract,
            Self::CodeEntryAbsent { .. } | Self::DigestContradiction { .. } => {
                ErrorCategory::Provenance
            },
            Self::UnrecognisedExecutable { .. } => ErrorCategory::Contract,
            Self::UnexpectedEntry { .. } => ErrorCategory::Network,
            Self::MalformedModule { .. } => ErrorCategory::Contract,
            Self::SpecSectionUndecodable { .. } => ErrorCategory::Contract,
        }
    }

    /// Whether the engine learned something definite about the contract's absence.
    ///
    /// True only for [`InspectionFailure::NotDeployed`]. A caller deciding whether
    /// to report "the contract is not there" must ask this rather than match on the
    /// variant, so that the question has one answer in one place.
    #[must_use]
    pub const fn is_absence(&self) -> bool {
        matches!(self, Self::NotDeployed { .. })
    }

    /// Whether this failure means the engine holds two facts that cannot both be
    /// true.
    #[must_use]
    pub const fn is_contradiction(&self) -> bool {
        matches!(self, Self::DigestContradiction { .. })
    }

    /// Converts the failure into the engine's structured error.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        match self {
            Self::NotDeployed {
                contract_id,
                network,
                ledger,
            } => EngineError::ContractNotFound {
                contract_id,
                network,
                ledger: ledger.get(),
            },
            Self::CodeEntryAbsent {
                contract_id,
                wasm_hash,
            } => EngineError::Provenance(format!(
                "contract {contract_id} names WASM {wasm_hash}, but no code entry for that hash \
                 exists at the observed boundary; the contract cannot be analysed from its \
                 executable. This is not an absence of the contract"
            )),
            Self::UnrecognisedExecutable { contract_id } => EngineError::Contract(format!(
                "contract {contract_id} exists but its executable is neither a WASM module nor a \
                 Stellar Asset Contract, so the engine cannot determine what it runs"
            )),
            Self::DigestContradiction {
                contract_id,
                recorded,
                computed,
            } => EngineError::Provenance(format!(
                "contract {contract_id} records executable hash {recorded}, but the returned \
                 module hashes to {computed}; these two observations cannot both be correct"
            )),
            Self::UnexpectedEntry { expected, received } => EngineError::MalformedResponse {
                endpoint: String::new(),
                detail: format!("expected {expected}, but the endpoint returned {received}"),
            },
            Self::MalformedModule { detail } => EngineError::Contract(format!(
                "the deployed executable is not a well-formed WebAssembly module: {detail}"
            )),
            Self::SpecSectionUndecodable { detail } => EngineError::Contract(format!(
                "the module's contract specification section could not be decoded: {detail}. The \
                 module remains verifiable by digest; only its declared interface is unreadable"
            )),
        }
    }
}

/// A one-line description of what went wrong, without the identifiers.
///
/// Separate from the `Display` of the resulting [`EngineError`] because a report
/// often has the identifiers in its own columns and would otherwise repeat them.
#[must_use]
pub const fn describe(failure: &InspectionFailure) -> &'static str {
    match failure {
        InspectionFailure::NotDeployed { .. } => "no contract is deployed at this address",
        InspectionFailure::CodeEntryAbsent { .. } => {
            "the executable named by the instance is absent"
        },
        InspectionFailure::UnrecognisedExecutable { .. } => "the executable kind is not recognised",
        InspectionFailure::DigestContradiction { .. } => {
            "the module does not hash to the recorded executable hash"
        },
        InspectionFailure::UnexpectedEntry { .. } => "the endpoint returned a different entry",
        InspectionFailure::MalformedModule { .. } => "the executable is not a valid module",
        InspectionFailure::SpecSectionUndecodable { .. } => {
            "the contract specification section is undecodable"
        },
    }
}

/// Turns the first of several failures into a `Result`, or succeeds when the list
/// is empty.
///
/// Used where inspection collects everything it could not do and reports once, so
/// that a caller sees all of a contract's problems rather than one per run.
///
/// # Errors
///
/// Returns the first failure's error when `failures` is non-empty.
pub fn first_failure(failures: Vec<InspectionFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> LedgerSequence {
        LedgerSequence::new(1000).expect("a real ledger")
    }

    #[test]
    fn a_deployed_absence_is_category_contract_not_a_transport_failure() {
        // The distinction the whole engine rests on: "the network answered, and the
        // answer is that nothing is there" is not the same as "the request failed".
        let failure = InspectionFailure::NotDeployed {
            contract_id: "C".to_owned() + &"A".repeat(55),
            network: "testnet".to_owned(),
            ledger: ledger(),
        };
        assert_eq!(failure.category(), ErrorCategory::Contract);
        assert!(failure.is_absence());
        match failure.clone().into_error() {
            EngineError::ContractNotFound {
                network,
                ledger: at,
                ..
            } => {
                assert_eq!(network, "testnet");
                assert_eq!(at, 1000);
            },
            other => panic!("expected a not-found error, got {other}"),
        }
        // And it must not be retryable: asking again will produce the same answer.
        assert!(!failure.into_error().retryable());
    }

    #[test]
    fn a_missing_code_entry_is_a_provenance_anomaly_not_an_absence() {
        // Conflating the two would let a contract whose executable was pruned be
        // reported as though it had never been deployed.
        let failure = InspectionFailure::CodeEntryAbsent {
            contract_id: "C".to_owned() + &"A".repeat(55),
            wasm_hash: "a".repeat(64),
        };
        assert_eq!(failure.category(), ErrorCategory::Provenance);
        assert!(
            !failure.is_absence(),
            "the contract exists; its executable does not"
        );
        let message = failure.into_error().to_string();
        assert!(
            message.contains("not an absence"),
            "the error must say so explicitly so a reader does not read it as not-found: {message}"
        );
    }

    #[test]
    fn a_digest_contradiction_is_reported_as_a_contradiction() {
        let failure = InspectionFailure::DigestContradiction {
            contract_id: "C".to_owned() + &"A".repeat(55),
            recorded: "a".repeat(64),
            computed: "b".repeat(64),
        };
        assert!(failure.is_contradiction());
        assert_eq!(failure.category(), ErrorCategory::Provenance);
        let message = failure.clone().into_error().to_string();
        assert!(message.contains(&"a".repeat(64)));
        assert!(message.contains(&"b".repeat(64)));
        assert!(!failure.is_absence());
    }

    #[test]
    fn an_undecodable_spec_section_does_not_claim_the_module_is_malformed() {
        // The module is still verifiable by digest; overstating the problem would
        // hide a verification that still holds.
        let failure = InspectionFailure::SpecSectionUndecodable {
            detail: "trailing bytes after entry 3".to_owned(),
        };
        assert_eq!(failure.category(), ErrorCategory::Contract);
        assert!(!failure.is_contradiction());
        let message = failure.into_error().to_string();
        assert!(
            message.contains("remains verifiable by digest"),
            "the error must not deny the verification that still holds: {message}"
        );
    }

    #[test]
    fn every_failure_has_a_category_and_a_description() {
        let failures = vec![
            InspectionFailure::NotDeployed {
                contract_id: "C".to_owned() + &"A".repeat(55),
                network: "testnet".to_owned(),
                ledger: ledger(),
            },
            InspectionFailure::CodeEntryAbsent {
                contract_id: "C".to_owned() + &"A".repeat(55),
                wasm_hash: "a".repeat(64),
            },
            InspectionFailure::UnrecognisedExecutable {
                contract_id: "C".to_owned() + &"A".repeat(55),
            },
            InspectionFailure::DigestContradiction {
                contract_id: "C".to_owned() + &"A".repeat(55),
                recorded: "a".repeat(64),
                computed: "b".repeat(64),
            },
            InspectionFailure::UnexpectedEntry {
                expected: "a contract data entry".to_owned(),
                received: "a contract code entry".to_owned(),
            },
            InspectionFailure::MalformedModule {
                detail: "bad magic".to_owned(),
            },
            InspectionFailure::SpecSectionUndecodable {
                detail: "truncated".to_owned(),
            },
        ];

        for failure in &failures {
            // Every category is one the specification enumerates, and every
            // description is non-empty, so no failure can reach a report blank.
            assert!(
                ErrorCategory::all().contains(&failure.category()),
                "{failure:?} has a category outside the specification's enumeration"
            );
            assert!(!describe(failure).is_empty());
            assert!(!failure.clone().into_error().code().is_empty());
        }
        assert_eq!(failures.len(), 7, "every variant is covered by this test");
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![InspectionFailure::MalformedModule {
            detail: "bad magic".to_owned(),
        }])
        .expect_err("a failure must be reported");
        assert!(error.to_string().contains("bad magic"));
    }
}
