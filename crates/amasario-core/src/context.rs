//! The execution context.
//!
//! The context carries everything that could change a run's result, and it is
//! passed explicitly rather than read from the environment. That is not a style
//! preference: the specification requires the engine's output to be deterministic
//! for a given input, specification version, observation boundary, available
//! evidence and engine version, and anything a helper could read ambiently - the
//! clock, the environment, a global - would let two runs differ with no record of
//! why.
//!
//! The context also carries the cancellation signal. Analysis performs network
//! requests with a bounded depth, and a caller that cancels must get a result that
//! says it was cancelled rather than a shorter result. `TruncationReason::Cancelled`
//! exists for that, and this module is where the signal comes from.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::configuration::EngineConfig;
use crate::errors::{EngineError, Result};
use crate::identity::LedgerSequence;
use crate::observations::{Network, ObservationBoundary};

/// A cooperative cancellation signal.
///
/// Shared rather than owned so that a caller holding one can cancel a run it does
/// not control. The check is a plain atomic load: cancellation is polled between
/// requests, not inside them, because a request that is already in flight finishes
/// or times out on its own and an abort mid-response would leave the engine unable
/// to say whether the response was received.
#[derive(Debug, Clone, Default)]
pub struct Cancellation {
    flag: Arc<AtomicBool>,
}

impl Cancellation {
    /// A fresh, uncancelled signal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Returns an error if cancellation has been requested.
    ///
    /// Called before each unit of work, so that a cancelled run stops at a defined
    /// boundary and can report that it stopped rather than appearing to have
    /// finished.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(EngineError::Graph(
                "the operation was cancelled before it completed".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Everything that qualifies one engine run.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// The configuration in force.
    config: EngineConfig,
    /// The network the run is observing.
    network: Network,
    /// The ledger the run's observations are bounded by.
    boundary_ledger: LedgerSequence,
    /// When the run began, as an RFC 3339 timestamp.
    ///
    /// Supplied by the caller rather than read from the clock, so that a test can
    /// pin it and so that two runs with the same input produce the same output.
    /// This is the field the specification excludes from every digest.
    started_at: String,
    /// The cancellation signal.
    cancellation: Cancellation,
}

impl ExecutionContext {
    /// Builds a context, validating the configuration.
    ///
    /// Validation happens here rather than at first use so that a run which cannot
    /// succeed fails before it contacts anything, and so that the error names the
    /// configuration problem rather than surfacing later as an empty result.
    pub fn new(
        config: EngineConfig,
        network: Network,
        boundary_ledger: LedgerSequence,
        started_at: impl Into<String>,
    ) -> Result<Self> {
        config.validate()?;
        config.ensure_bounded()?;
        let started_at = started_at.into();
        if started_at.is_empty() {
            return Err(EngineError::Configuration(
                "an execution context needs the time the run began; it is excluded from \
                 every digest but a report must be able to state it"
                    .to_owned(),
            ));
        }
        Ok(Self {
            config,
            network,
            boundary_ledger,
            started_at,
            cancellation: Cancellation::new(),
        })
    }

    /// Attaches a cancellation signal the caller controls.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: Cancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// The configuration in force.
    #[must_use]
    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// The network being observed.
    #[must_use]
    pub const fn network(&self) -> &Network {
        &self.network
    }

    /// The ledger the run is bounded by.
    #[must_use]
    pub const fn boundary_ledger(&self) -> LedgerSequence {
        self.boundary_ledger
    }

    /// When the run began.
    #[must_use]
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    /// The cancellation signal.
    #[must_use]
    pub const fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }

    /// Builds an observation boundary from this context.
    ///
    /// The single place a boundary is constructed, so that no observation can be
    /// recorded against a boundary the context did not authorise.
    #[must_use]
    pub fn boundary(&self) -> ObservationBoundary {
        ObservationBoundary::new(
            self.network.clone(),
            self.boundary_ledger,
            self.started_at.clone(),
        )
    }

    /// Returns an error if cancellation has been requested.
    pub fn check_cancelled(&self) -> Result<()> {
        self.cancellation.check()
    }

    /// Whether the engine may inspect source and build inputs.
    #[must_use]
    pub const fn inspects_sources(&self) -> bool {
        self.config.inspect_sources
    }

    /// The depth bound in force.
    #[must_use]
    pub const fn max_depth(&self) -> u32 {
        self.config.bounds.max_depth
    }

    /// The node bound in force.
    #[must_use]
    pub const fn max_nodes(&self) -> usize {
        self.config.bounds.max_nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observations::NetworkType;

    fn testnet() -> Network {
        Network::new(
            "testnet",
            NetworkType::Testnet,
            "Test SDF Network ; September 2015",
        )
        .expect("a well-formed descriptor")
    }

    fn context() -> ExecutionContext {
        ExecutionContext::new(
            EngineConfig::default(),
            testnet(),
            LedgerSequence::new(1234567).expect("a real ledger"),
            "2026-09-15T00:00:00Z",
        )
        .expect("a valid context")
    }

    #[test]
    fn a_context_carries_the_configuration_the_network_and_the_boundary() {
        let context = context();
        assert_eq!(context.network().id, "testnet");
        assert_eq!(context.boundary_ledger().get(), 1234567);
        assert_eq!(context.started_at(), "2026-09-15T00:00:00Z");
        assert_eq!(context.max_depth(), crate::configuration::DEFAULT_MAX_DEPTH);
    }

    #[test]
    fn the_boundary_a_context_produces_is_the_one_it_was_constructed_with() {
        let context = context();
        let boundary = context.boundary();
        // A single construction point means an observation cannot be recorded
        // against a boundary the context did not authorise.
        assert_eq!(boundary.ledger, context.boundary_ledger());
        assert_eq!(boundary.observed_at, context.started_at());
        assert_eq!(boundary.network.id, context.network().id);
    }

    #[test]
    fn an_invalid_configuration_is_refused_before_the_run_starts() {
        let config = EngineConfig {
            concurrency: 0,
            ..EngineConfig::default()
        };
        let error = ExecutionContext::new(
            config,
            testnet(),
            LedgerSequence::new(1).expect("valid"),
            "2026-09-15T00:00:00Z",
        )
        .expect_err("a context with an unusable configuration must not be built");
        assert_eq!(
            error.category(),
            crate::errors::ErrorCategory::Configuration
        );
    }

    #[test]
    fn a_context_without_a_start_time_is_refused() {
        ExecutionContext::new(
            EngineConfig::default(),
            testnet(),
            LedgerSequence::new(1).expect("valid"),
            "",
        )
        .expect_err("a run must be able to state when it began");
    }

    #[test]
    fn cancellation_is_pollable_and_reports_the_interruption() {
        let context = context();
        context.check_cancelled().expect("not yet cancelled");
        assert!(!context.cancellation().is_cancelled());

        context.cancellation().cancel();
        assert!(context.cancellation().is_cancelled());
        let error = context
            .check_cancelled()
            .expect_err("a cancelled run must not continue silently");
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn cancelling_twice_is_idempotent() {
        let cancellation = Cancellation::new();
        cancellation.cancel();
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn a_shared_cancellation_signal_stops_a_run_its_owner_does_not_control() {
        let cancellation = Cancellation::new();
        let context = context().with_cancellation(cancellation.clone());
        // The clone is what a supervisor holds; cancelling through it must reach
        // the context, which is the whole point of sharing the flag.
        cancellation.cancel();
        assert!(context.cancellation().is_cancelled());
    }

    #[test]
    fn a_context_reports_whether_sources_may_be_inspected() {
        let context = ExecutionContext::new(
            EngineConfig {
                inspect_sources: false,
                ..EngineConfig::default()
            },
            testnet(),
            LedgerSequence::new(1).expect("valid"),
            "2026-09-15T00:00:00Z",
        )
        .expect("valid");
        assert!(!context.inspects_sources());
    }
}
