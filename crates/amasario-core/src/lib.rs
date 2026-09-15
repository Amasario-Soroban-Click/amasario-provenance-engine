//! Execution context, observation model, relationship semantics and pipeline
//! coordination for the Amasario provenance engine.
//!
//! # What this crate is
//!
//! AMASARIO answers one question: *what does this Soroban contract depend on, where
//! did its deployed artifact come from, what evidence supports those relationships,
//! and what could be affected when something upstream changes?*
//!
//! This crate holds the vocabulary and the coordination for that answer. It does not
//! contact a network, read a contract, build a graph, or render a report - those are
//! the responsibilities of the other crates in the workspace, each of which owns its
//! own dependencies. Keeping them out of here is what allows `amasario-core` to sit
//! at the bottom of the dependency graph and be depended upon by everything.
//!
//! # The four ideas this crate exists to enforce
//!
//! **An address is not an identity.** [`identity::ContractId`] is a validated
//! address, and [`identity::EntityKind`] distinguishes it from the executable it
//! hosts. Nothing here can represent "the contract at `C...`" as a complete identity,
//! because a contract can be upgraded and its address survives unchanged.
//!
//! **A claim without evidence is not a claim.**
//! [`relationships::Confidence::new`] refuses an empty evidence list, so there is no
//! way to construct a confidence level that names nothing. `VERIFIED` therefore
//! describes evidence completeness, never the trustworthiness of a contract.
//!
//! **A contradiction must be representable.** [`relationships::VerificationStatus`]
//! has a `Conflicting` variant that takes precedence over every other status. Without
//! it, a claimed source revision that rebuilds to a different digest from the
//! deployed module would have to be reported as `VERIFIED` or discarded, and an
//! incorrect `VERIFIED` is the most damaging output this system can produce.
//!
//! **A bounded search must say it was bounded.** [`observations::TruncationReason`]
//! exists so that "no dependency was found" and "the search stopped" are different
//! results. A consumer cannot tell them apart after the fact, so the engine records
//! which one happened.
//!
//! # What the engine does not claim
//!
//! Amasario models dependency, provenance and impact relationships. It is **not** a
//! security scanner and no output is a security opinion. No term in this crate means
//! "secure", "safe", "malicious" or "vulnerable", and `VERIFIED` is a statement
//! about the relationship between evidence and a claim rather than a property of a
//! contract. A fully verified provenance chain can describe a deliberate backdoor.
//!
//! # Determinism
//!
//! Analysis results are deterministic for the same input, specification version,
//! observation boundary, available evidence and engine version. That requirement is
//! why:
//!
//! * [`context::ExecutionContext`] takes its start time as an argument rather than
//!   reading the clock, and carries everything else a run depends on;
//! * iteration over anything that reaches output uses ordered collections, which
//!   `clippy.toml` enforces by refusing `HashMap` and `HashSet`;
//! * a configuration is validated before a run starts, so an unusable bound fails
//!   with a named problem rather than producing a shorter result.
//!
//! # Example
//!
//! ```no_run
//! use amasario_core::{
//!     configuration::EngineConfig,
//!     engine::Engine,
//!     identity::LedgerSequence,
//!     observations::{Network, NetworkType},
//!     pipeline::{Pipeline, Stage},
//! };
//!
//! # fn main() -> amasario_core::errors::Result<()> {
//! let engine = Engine::new(EngineConfig::with_depth(4)?)?;
//!
//! let context = engine.begin_run(
//!     Network::new("testnet", NetworkType::Testnet, "Test SDF Network ; September 2015")?,
//!     LedgerSequence::new(1_234_567)?,
//!     "2026-09-15T00:00:00Z",
//! )?;
//!
//! let mut pipeline = Pipeline::new(&context);
//! pipeline.run(Stage::LoadConfiguration, |_| Ok(()))?;
//! // ... the remaining stages are supplied by the crates that own them.
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

// Lint policy lives in `[workspace.lints]` in the workspace manifest rather than
// here, so that all twelve crates are held to the same standard and a relaxation
// has to be made in one visible place. `clippy::pedantic` is deliberately not
// enabled wholesale: it contains lints whose advice conflicts with the clarity
// this codebase needs - `missing_errors_doc` on every `Result`-returning helper,
// for instance - and enabling it would mean scattering `allow` attributes until
// the genuinely useful lints were buried.

pub mod configuration;
pub mod context;
pub mod engine;
pub mod errors;
pub mod identity;
pub mod observations;
pub mod pipeline;
pub mod relationships;

// Re-exported together because they are used together: a caller building a run
// needs the configuration, the bounds it contains, and the policy that governs
// retries, and having to know which module each lives in would be friction with no
// benefit. The constants travel with them so that a caller can name a default
// rather than restating a number.
pub use configuration::{
    DEFAULT_CONCURRENCY, DEFAULT_MAX_ATTEMPTS, DEFAULT_MAX_DEPTH, DEFAULT_MAX_NODES,
    DEFAULT_REQUEST_TIMEOUT, DEFAULT_RETRY_BACKOFF, DepthBounds, EngineConfig,
    MAX_PERMITTED_CONCURRENCY, MAX_PERMITTED_DEPTH, RecursionMode, RetryPolicy,
};
pub use context::{Cancellation, ExecutionContext};
pub use engine::{ENGINE_VERSION, Engine, SUPPORTED_API_VERSION, SUPPORTED_SPEC_VERSION};
pub use errors::{EngineError, ErrorCategory, Result};
pub use identity::{
    ContractId, Digest, DigestAlgorithm, EntityKind, EntityRef, LedgerSequence, TransactionHash,
};
pub use observations::{
    Network, NetworkType, Observation, ObservationBoundary, ObservationProvenance,
    TraversalOutcome, TruncationReason,
};
pub use pipeline::{Pipeline, PipelineReport, Stage, StageOutcome};
pub use relationships::{
    Basis, ChangePropagation, Confidence, ConfidenceLevel, DependencyClass, EvidenceType,
    Relationship, VerificationStatus,
};

/// The specification's `apiVersion` for the version this engine implements.
///
/// Re-exported at the crate root so that a consumer can check compatibility without
/// knowing which module the constant lives in, which keeps the check a single
/// obvious call rather than something a caller has to discover.
pub const API_VERSION: &str = SUPPORTED_API_VERSION;

/// The specification's `specVersion` for the version this engine implements.
pub const SPEC_VERSION: &str = SUPPORTED_SPEC_VERSION;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to
        // name it, and a re-export that went missing would be a breaking change
        // discovered by a downstream crate rather than here.
        assert_eq!(API_VERSION, SUPPORTED_API_VERSION);
        assert_eq!(SPEC_VERSION, SUPPORTED_SPEC_VERSION);
        assert_eq!(EntityKind::Contract.as_str(), "CONTRACT");
        assert_eq!(Relationship::DependsOn.as_str(), "DEPENDS_ON");
        assert_eq!(Basis::InferredInterface.as_str(), "INFERRED_INTERFACE");
        assert_eq!(ConfidenceLevel::Verified.as_str(), "VERIFIED");
        assert_eq!(VerificationStatus::Conflicting.as_str(), "CONFLICTING");
        assert_eq!(DependencyClass::Runtime.as_str(), "RUNTIME");
        assert_eq!(EvidenceType::Attestation.as_str(), "ATTESTATION");
        assert_eq!(NetworkType::Mainnet.as_str(), "MAINNET");
        assert_eq!(TruncationReason::RateLimited.as_str(), "RATE_LIMITED");
        assert_eq!(Stage::AnalyseImpact.as_str(), "analyse-impact");
    }

    #[test]
    fn the_crate_exposes_the_engine_and_its_version() {
        let engine = Engine::with_defaults().expect("the default configuration is valid");
        assert_eq!(engine.version(), ENGINE_VERSION);
        assert_eq!(engine.spec_version(), SPEC_VERSION);
    }
}
