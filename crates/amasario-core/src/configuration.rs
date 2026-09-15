//! Engine configuration.
//!
//! Configuration is an input, never ambient state. The engine's output is required
//! to be deterministic for a given input, specification version, observation
//! boundary, available evidence and engine version, so anything that could change
//! a result has to be visible in the type a caller passes in. A configuration read
//! from the environment inside a helper would make two runs on the same input
//! differ with no record of why.
//!
//! Every bound has a default that is safe rather than permissive. In particular
//! [`DepthBounds::max_depth`] is bounded and there is no "unbounded" value: the
//! specification forbids unbounded recursive network requests, and a value meaning
//! "no limit" would be the easiest possible way to violate that.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::errors::{EngineError, Result};

/// The deepest traversal the engine will perform unless told otherwise.
///
/// Six is enough to cover the chains seen in practice - a contract, its contract
/// dependencies, their dependencies, and the source and build behind them - while
/// keeping the number of network requests bounded at a level a public endpoint
/// will tolerate.
pub const DEFAULT_MAX_DEPTH: u32 = 6;

/// The largest number of nodes a traversal may accumulate before it stops.
///
/// This is a second bound, independent of depth, because a graph can be shallow and
/// enormous: a contract invoked by thousands of callers is depth one and would
/// still be unbounded work. A single bound is not enough, which is why the
/// specification's truncation reasons distinguish `MAX_DEPTH_REACHED` from
/// `MAX_NODES_REACHED`.
pub const DEFAULT_MAX_NODES: usize = 10_000;

/// The default per-request timeout.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The default number of attempts for a transient network failure.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// The default delay before the first retry.
pub const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// The maximum number of concurrent in-flight requests.
///
/// One by default. A public RPC endpoint rate limits per client, and a burst of
/// concurrent requests from an analysis tool is the fastest way to be throttled -
/// which produces a truncated result that a inattentive consumer would read as a
/// complete one. Raising this is possible and explicit.
pub const DEFAULT_CONCURRENCY: usize = 1;

/// Configuration for one engine run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// How far traversal may go before it must stop and say so.
    pub bounds: DepthBounds,
    /// How network requests are attempted and retried.
    pub retry: RetryPolicy,
    /// How many requests may be in flight at once.
    pub concurrency: usize,
    /// Whether to compute the full dependency graph or stop after direct edges.
    pub recursion: RecursionMode,
    /// Whether the engine may inspect source and build inputs at all.
    pub inspect_sources: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            bounds: DepthBounds::default(),
            retry: RetryPolicy::default(),
            concurrency: DEFAULT_CONCURRENCY,
            recursion: RecursionMode::Transitive,
            inspect_sources: true,
        }
    }
}

impl EngineConfig {
    /// Validates the configuration, collecting every problem rather than the first.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] naming every problem found, so that a
    /// caller fixing an invocation sees the whole list rather than one item per run.
    pub fn validate(&self) -> Result<()> {
        let mut problems = crate::errors::ErrorCollection::new();

        problems.check(
            self.bounds.max_depth >= 1,
            "/bounds/maxDepth",
            "a depth of at least 1 is required to observe anything at all",
        );
        problems.check(
            self.bounds.max_depth <= MAX_PERMITTED_DEPTH,
            "/bounds/maxDepth",
            &format!(
                "the depth bound may not exceed {MAX_PERMITTED_DEPTH}; an unbounded traversal is prohibited"
            ),
        );
        problems.check(
            self.bounds.max_nodes >= 1,
            "/bounds/maxNodes",
            "a node bound of at least 1 is required",
        );
        problems.check(
            self.concurrency >= 1,
            "/concurrency",
            "concurrency must be at least 1",
        );
        problems.check(
            self.concurrency <= MAX_PERMITTED_CONCURRENCY,
            "/concurrency",
            &format!("concurrency may not exceed {MAX_PERMITTED_CONCURRENCY}"),
        );
        problems.check(
            self.retry.max_attempts >= 1,
            "/retry/maxAttempts",
            "at least one attempt is required; a policy of zero attempts cannot observe anything",
        );
        problems.check(
            !self.retry.timeout.is_zero(),
            "/retry/timeout",
            "a zero timeout would fail every request without contacting the network",
        );

        // Reported as a configuration failure rather than a validation failure.
        // The specification's error model distinguishes the two because a caller
        // acts differently on them: a configuration problem means the invocation
        // was wrong and no retry can help, while a validation problem is about the
        // content being analysed.
        if problems.is_empty() {
            return Ok(());
        }
        let first = problems.errors()[0].to_string();
        Err(EngineError::Configuration(format!(
            "{} problem(s) found; first: {first}",
            problems.len()
        )))
    }
}

/// The largest depth bound a caller may request.
///
/// Not a matter of taste: each additional hop can multiply the number of network
/// requests, and the specification forbids unbounded recursive requests. A bound
/// is therefore always in force, and this is the largest one available.
pub const MAX_PERMITTED_DEPTH: u32 = 32;

/// The largest concurrency a caller may request.
pub const MAX_PERMITTED_CONCURRENCY: usize = 32;

/// How far a traversal may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepthBounds {
    /// The maximum number of edges between the starting entity and any reached
    /// entity.
    pub max_depth: u32,
    /// The maximum number of entities a traversal may accumulate.
    pub max_nodes: usize,
}

impl Default for DepthBounds {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_nodes: DEFAULT_MAX_NODES,
        }
    }
}

impl DepthBounds {
    /// Bounds of a given depth, with the default node limit.
    pub fn of_depth(max_depth: u32) -> Self {
        Self {
            max_depth,
            ..Self::default()
        }
    }
}

/// Whether a traversal follows dependency edges beyond the direct ones.
///
/// The default is [`RecursionMode::Transitive`], stated on the variant rather than
/// written as a hand-rolled `Default` implementation, so that the default is
/// visible where the variants are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecursionMode {
    /// Only the edges incident to the starting entity.
    ///
    /// Used where a caller wants an answer that is cheap and certain rather than
    /// complete, for example in a CI gate that only cares whether a direct
    /// dependency changed.
    Direct,
    /// Follow edges up to the configured depth bound.
    #[default]
    Transitive,
}

impl RecursionMode {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "DIRECT",
            Self::Transitive => "TRANSITIVE",
        }
    }
}

/// How network requests are attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// The timeout applied to a single attempt.
    pub timeout: Duration,
    /// The maximum number of attempts, including the first.
    pub max_attempts: u32,
    /// The delay before the first retry. Subsequent delays double.
    pub backoff: Duration,
    /// The ceiling on the backoff, so that a long-lived run cannot wait for hours.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_REQUEST_TIMEOUT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: DEFAULT_RETRY_BACKOFF,
            max_backoff: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// The delay before attempt number `attempt`, counting the first attempt as 1.
    ///
    /// Capped at [`RetryPolicy::max_backoff`]. Computed from the attempt index
    /// rather than from accumulated state so that two runs which fail at the same
    /// point wait the same amount, which is part of what makes a run reproducible.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::ZERO;
        }
        let doublings = attempt - 2;
        let multiplier = 1u32.checked_shl(doublings).unwrap_or(u32::MAX);
        self.backoff
            .saturating_mul(multiplier)
            .min(self.max_backoff)
    }

    /// Whether a further attempt is permitted after `attempt` attempts have been
    /// made.
    #[must_use]
    pub const fn may_retry_after(&self, attempt: u32) -> bool {
        attempt < self.max_attempts
    }
}

impl EngineConfig {
    /// Builds a configuration with an explicit depth bound and everything else
    /// defaulted, which is the shape the CLI's `--depth` flag produces.
    pub fn with_depth(max_depth: u32) -> Result<Self> {
        let config = Self {
            bounds: DepthBounds::of_depth(max_depth),
            ..Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    /// Builds a direct-only configuration.
    #[must_use]
    pub fn direct_only() -> Self {
        Self {
            recursion: RecursionMode::Direct,
            bounds: DepthBounds::of_depth(1),
            ..Self::default()
        }
    }

    /// Rejects any configuration that would require an unbounded traversal.
    ///
    /// Present as a separate call so that a caller which overrides the config
    /// programmatically - rather than through the CLI, which validates - cannot
    /// bypass the prohibition.
    pub fn ensure_bounded(&self) -> Result<()> {
        if self.bounds.max_depth > MAX_PERMITTED_DEPTH {
            return Err(EngineError::Configuration(format!(
                "depth bound {} exceeds the maximum of {MAX_PERMITTED_DEPTH}",
                self.bounds.max_depth
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_configuration_is_valid() {
        EngineConfig::default()
            .validate()
            .expect("the default configuration must be usable");
    }

    #[test]
    fn the_default_traversal_is_bounded() {
        let config = EngineConfig::default();
        // The specification forbids unbounded recursive network requests, so there
        // is no configuration in which a bound is absent.
        assert!(config.bounds.max_depth >= 1);
        assert!(config.bounds.max_depth <= MAX_PERMITTED_DEPTH);
        assert!(config.bounds.max_nodes >= 1);
        config.ensure_bounded().expect("default is bounded");
    }

    #[test]
    fn a_depth_bound_beyond_the_permitted_maximum_is_rejected() {
        let config = EngineConfig {
            bounds: DepthBounds::of_depth(MAX_PERMITTED_DEPTH + 1),
            ..EngineConfig::default()
        };
        let error = config.validate().expect_err("the bound is too large");
        assert!(error.to_string().contains("may not exceed"));
        assert!(config.ensure_bounded().is_err());
    }

    #[test]
    fn a_zero_depth_bound_is_rejected_because_it_could_observe_nothing() {
        let config = EngineConfig {
            bounds: DepthBounds::of_depth(0),
            ..EngineConfig::default()
        };
        let error = config
            .validate()
            .expect_err("a depth of 0 observes nothing");
        assert!(error.to_string().contains("at least 1"));
    }

    #[test]
    fn a_zero_concurrency_is_rejected() {
        let config = EngineConfig {
            concurrency: 0,
            ..EngineConfig::default()
        };
        config
            .validate()
            .expect_err("concurrency of 0 cannot make a request");
    }

    #[test]
    fn concurrency_is_bounded_above() {
        let config = EngineConfig {
            concurrency: MAX_PERMITTED_CONCURRENCY + 1,
            ..EngineConfig::default()
        };
        config.validate().expect_err("concurrency is bounded");
    }

    #[test]
    fn a_retry_policy_of_zero_attempts_is_rejected() {
        let config = EngineConfig {
            retry: RetryPolicy {
                max_attempts: 0,
                ..RetryPolicy::default()
            },
            ..EngineConfig::default()
        };
        config
            .validate()
            .expect_err("zero attempts observes nothing");
    }

    #[test]
    fn a_zero_timeout_is_rejected() {
        let config = EngineConfig {
            retry: RetryPolicy {
                timeout: Duration::ZERO,
                ..RetryPolicy::default()
            },
            ..EngineConfig::default()
        };
        config
            .validate()
            .expect_err("a zero timeout cannot contact the network");
    }

    #[test]
    fn every_configuration_problem_is_reported_together() {
        let config = EngineConfig {
            bounds: DepthBounds {
                max_depth: 0,
                max_nodes: 0,
            },
            concurrency: 0,
            retry: RetryPolicy {
                max_attempts: 0,
                timeout: Duration::ZERO,
                ..RetryPolicy::default()
            },
            ..EngineConfig::default()
        };
        let error = config
            .validate()
            .expect_err("four problems were introduced");
        // A validator that stopped at the first problem would make a contributor
        // fix one issue per run and would hide how many remain.
        assert!(error.to_string().contains("5 problem(s)"));
    }

    #[test]
    fn the_first_retry_is_immediate_and_later_retries_back_off() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for_attempt(1), Duration::ZERO);
        assert_eq!(policy.delay_for_attempt(2), policy.backoff);
        assert_eq!(
            policy.delay_for_attempt(3),
            policy.backoff.saturating_mul(2)
        );
        assert_eq!(
            policy.delay_for_attempt(4),
            policy.backoff.saturating_mul(4)
        );
    }

    #[test]
    fn the_backoff_is_capped_so_a_long_run_cannot_wait_for_hours() {
        let policy = RetryPolicy {
            backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(8),
            max_attempts: 64,
            ..RetryPolicy::default()
        };
        assert_eq!(policy.delay_for_attempt(32), Duration::from_secs(8));
        assert_eq!(policy.delay_for_attempt(64), Duration::from_secs(8));
    }

    #[test]
    fn the_retry_decision_stops_at_the_configured_attempt_limit() {
        let policy = RetryPolicy {
            max_attempts: 3,
            ..RetryPolicy::default()
        };
        assert!(policy.may_retry_after(1));
        assert!(policy.may_retry_after(2));
        assert!(!policy.may_retry_after(3));
    }

    #[test]
    fn a_configuration_is_reproducible_through_its_serialised_form() {
        let config = EngineConfig {
            bounds: DepthBounds {
                max_depth: 4,
                max_nodes: 250,
            },
            retry: RetryPolicy {
                timeout: Duration::from_secs(5),
                max_attempts: 2,
                backoff: Duration::from_millis(100),
                max_backoff: Duration::from_secs(10),
            },
            concurrency: 2,
            recursion: RecursionMode::Direct,
            inspect_sources: false,
        };
        let encoded = serde_json::to_string(&config).expect("configuration serialises");
        let decoded: EngineConfig =
            serde_json::from_str(&encoded).expect("configuration deserialises");
        assert_eq!(decoded, config);
    }

    #[test]
    fn the_direct_only_configuration_reaches_one_hop_and_no_further() {
        let config = EngineConfig::direct_only();
        assert_eq!(config.recursion, RecursionMode::Direct);
        assert_eq!(config.bounds.max_depth, 1);
        config
            .validate()
            .expect("a direct-only configuration is valid");
    }

    #[test]
    fn a_depth_convenience_constructor_rejects_an_impossible_depth() {
        EngineConfig::with_depth(3).expect("3 is a usable depth");
        EngineConfig::with_depth(0).expect_err("0 observes nothing");
        EngineConfig::with_depth(MAX_PERMITTED_DEPTH + 1).expect_err("the bound is enforced");
    }

    #[test]
    fn an_unknown_configuration_field_is_rejected_rather_than_ignored() {
        // A silently ignored setting is indistinguishable from a honoured one, and
        // the difference here is whether the run honoured the operator's bound.
        let json = r#"{"bounds":{"max_depth":3,"max_nodes":100},"retry":{"timeout":{"secs":5,"nanos":0},"max_attempts":2,"backoff":{"secs":0,"nanos":1000000},"max_backoff":{"secs":10,"nanos":0}},"concurrency":1,"recursion":"TRANSITIVE","inspect_sources":true,"unlimited_depth":true}"#;
        serde_json::from_str::<EngineConfig>(json).expect_err("unknown fields are not ignored");
    }
}
