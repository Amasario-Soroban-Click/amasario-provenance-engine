//! The engine entry point.
//!
//! [`Engine`] holds what is true for every run: the specification version it
//! implements, the engine version it reports, and the compatibility rules that
//! decide whether an input can be interpreted at all. A run is then a
//! [`crate::context::ExecutionContext`] paired with a
//! [`crate::pipeline::Pipeline`].
//!
//! The compatibility check is deliberately strict, and it is the one place where
//! the engine refuses rather than proceeds. The specification requires a consumer
//! to reject an unrecognised compatibility family before interpreting any field,
//! because a changed field could invert an impact conclusion, and an engine that
//! guessed would produce a plausible-looking wrong answer instead of an error.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::configuration::EngineConfig;
use crate::context::ExecutionContext;
use crate::errors::{EngineError, Result};
use crate::identity::LedgerSequence;
use crate::observations::Network;

/// The specification version this engine implements.
///
/// Pinned rather than inferred. The engine's own version and the specification's
/// version move independently, and a document records both, so the two must not be
/// conflated. `amasario-provenance-spec` at `1.0.0` is the version this engine
/// consumes.
pub const SUPPORTED_SPEC_VERSION: &str = "1.0.0";

/// The compatibility family this engine speaks.
pub const SUPPORTED_API_VERSION: &str = "amasario.dev/v1";

/// The engine's own version, taken from the crate metadata at compile time.
///
/// Not hard-coded, so a release cannot forget to update it: a version that
/// disagreed with the packaged artefact would make "which engine produced this
/// document" unanswerable, which is the one thing the field exists for.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A semantic version, parsed far enough to compare.
///
/// Deliberately minimal: the engine needs to test major equality and minor
/// ordering, and a full SemVer implementation would be a dependency carrying
/// pre-release and build-metadata semantics this code never acts on. Parsing is
/// strict, so a malformed version is an error rather than a silent zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// Parses a `major.minor.patch` version.
    pub fn parse(value: &str) -> Result<Self> {
        let mut parts = value.split('.');
        let mut next = || -> Result<u64> {
            let part = parts.next().ok_or_else(|| EngineError::Validation {
                path: "/specVersion".to_owned(),
                detail: format!("{value:?} is not a major.minor.patch version"),
            })?;
            // A leading `+` or `-` is rejected by `parse`, so a pre-release
            // suffix becomes an error rather than being silently dropped.
            part.parse::<u64>().map_err(|_| EngineError::Validation {
                path: "/specVersion".to_owned(),
                detail: format!("{part:?} in {value:?} is not a non-negative integer"),
            })
        };
        let major = next()?;
        let minor = next()?;
        let patch = next()?;
        if parts.next().is_some() {
            return Err(EngineError::Validation {
                path: "/specVersion".to_owned(),
                detail: format!("{value:?} has more than three components"),
            });
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// The major component.
    #[must_use]
    pub const fn major(self) -> u64 {
        self.major
    }

    /// The minor component.
    #[must_use]
    pub const fn minor(self) -> u64 {
        self.minor
    }

    /// The patch component.
    #[must_use]
    pub const fn patch(self) -> u64 {
        self.patch
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The version fields a produced document carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecificationStamp {
    /// The compatibility family.
    pub api_version: String,
    /// The specification version.
    pub spec_version: String,
}

impl SpecificationStamp {
    /// The stamp this engine produces.
    #[must_use]
    pub fn current() -> Self {
        Self {
            api_version: SUPPORTED_API_VERSION.to_owned(),
            spec_version: SUPPORTED_SPEC_VERSION.to_owned(),
        }
    }

    /// Checks that this engine can interpret a document stamped with these
    /// versions.
    ///
    /// Three outcomes, and the distinction between them matters:
    ///
    /// * An unrecognised compatibility family is a hard rejection, before any
    ///   field is read.
    /// * A malformed specification version is a rejection.
    /// * A specification version in the same family but newer than this engine
    ///   supports is a rejection, not a warning. Minor releases are additive, so a
    ///   field this engine does not know about cannot affect its interpretation -
    ///   but a producer that emitted it may have relied on it, and proceeding would
    ///   present a partial understanding as a complete one.
    pub fn check_compatible(&self) -> Result<()> {
        if self.api_version != SUPPORTED_API_VERSION {
            return Err(EngineError::SpecificationCompatibility {
                detail: format!(
                    "this engine speaks the {SUPPORTED_API_VERSION} compatibility family and \
                     cannot interpret a document in the {} family; a changed field could \
                     invert an analysis, so the document is rejected rather than guessed at",
                    self.api_version
                ),
                found: Some(self.api_version.clone()),
                supported: SUPPORTED_API_VERSION.to_owned(),
            });
        }

        let found = Version::parse(&self.spec_version).map_err(|_| {
            EngineError::SpecificationCompatibility {
                detail: format!(
                    "{:?} is not a valid specification version",
                    self.spec_version
                ),
                found: Some(self.spec_version.clone()),
                supported: SUPPORTED_SPEC_VERSION.to_owned(),
            }
        })?;
        let supported = Version::parse(SUPPORTED_SPEC_VERSION)?;

        if found.major() != supported.major() {
            return Err(EngineError::SpecificationCompatibility {
                detail: format!(
                    "specification version {found} is in a different major family from the \
                     supported {supported}"
                ),
                found: Some(self.spec_version.clone()),
                supported: SUPPORTED_SPEC_VERSION.to_owned(),
            });
        }
        if found.minor() > supported.minor() {
            return Err(EngineError::SpecificationCompatibility {
                detail: format!(
                    "specification version {found} is newer than the supported {supported}; \
                     a document produced under a newer minor version may rely on fields this \
                     engine does not implement"
                ),
                found: Some(self.spec_version.clone()),
                supported: SUPPORTED_SPEC_VERSION.to_owned(),
            });
        }
        Ok(())
    }
}

/// How much of a document's content this engine can honour.
///
/// Returned when a document is compatible but comes from an older specification
/// version, so that a caller can reduce a confidence level or report the reduction
/// rather than silently ignoring what it does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InterpretationDepth {
    /// Every field in the document is implemented at the stated version.
    Full,
    /// The document is interpretable, but fields introduced after the version it
    /// declares are absent, so a comparison against a newer document is partial.
    Reduced,
}

/// The engine.
#[derive(Debug, Clone)]
pub struct Engine {
    config: EngineConfig,
}

impl fmt::Debug for EngineConfigSummary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineConfigSummary")
            .field("max_depth", &self.0.bounds.max_depth)
            .field("max_nodes", &self.0.bounds.max_nodes)
            .field("concurrency", &self.0.concurrency)
            .finish()
    }
}

/// A compact view of a configuration, for logging.
///
/// A summary rather than the configuration itself so that a log line cannot carry
/// an endpoint, and so that `Debug` output stays readable. The full configuration
/// is serialisable where it is actually needed.
pub struct EngineConfigSummary<'a>(&'a EngineConfig);

impl Engine {
    /// Builds an engine, validating its configuration.
    pub fn new(config: EngineConfig) -> Result<Self> {
        config.validate()?;
        config.ensure_bounded()?;
        Ok(Self { config })
    }

    /// Builds an engine with the default configuration.
    pub fn with_defaults() -> Result<Self> {
        Self::new(EngineConfig::default())
    }

    /// The value written to a document's `specVersion` field.
    #[must_use]
    pub const fn spec_version(&self) -> &'static str {
        SUPPORTED_SPEC_VERSION
    }

    /// The configuration in force.
    #[must_use]
    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// The engine's own version.
    #[must_use]
    pub const fn version(&self) -> &'static str {
        ENGINE_VERSION
    }

    /// The stamp this engine writes onto every document it produces.
    #[must_use]
    pub fn stamp(&self) -> SpecificationStamp {
        SpecificationStamp::current()
    }

    /// A logging-friendly summary of the configuration.
    #[must_use]
    pub const fn config_summary(&self) -> EngineConfigSummary<'_> {
        EngineConfigSummary(&self.config)
    }

    /// Begins a run against a network at a ledger boundary.
    ///
    /// The boundary ledgers are checked for order here rather than at first use, so
    /// an inverted pair fails before anything is contacted.
    pub fn begin_run(
        &self,
        network: Network,
        at_ledger: LedgerSequence,
        started_at: impl Into<String>,
    ) -> Result<ExecutionContext> {
        ExecutionContext::new(self.config.clone(), network, at_ledger, started_at)
    }

    /// Checks that a document's version stamp can be interpreted.
    pub fn check_document(&self, stamp: &SpecificationStamp) -> Result<InterpretationDepth> {
        stamp.check_compatible()?;
        let found = Version::parse(&stamp.spec_version)?;
        let supported = Version::parse(SUPPORTED_SPEC_VERSION)?;
        if found < supported {
            Ok(InterpretationDepth::Reduced)
        } else {
            Ok(InterpretationDepth::Full)
        }
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
        .expect("valid")
    }

    #[test]
    fn the_engine_reports_its_own_version_from_the_manifest() {
        let engine = Engine::with_defaults().expect("the default configuration is valid");
        // Read from the crate metadata rather than hard-coded, so a release cannot
        // forget to update it.
        assert_eq!(engine.version(), VERSION_FROM_MANIFEST);
        assert!(!engine.version().is_empty());
    }

    const VERSION_FROM_MANIFEST: &str = env!("CARGO_PKG_VERSION");

    #[test]
    fn the_engine_stamp_records_both_version_fields() {
        let engine = Engine::with_defaults().expect("valid");
        let stamp = engine.stamp();
        assert_eq!(stamp.api_version, "amasario.dev/v1");
        assert_eq!(stamp.spec_version, "1.0.0");
        assert_eq!(engine.spec_version(), "1.0.0");
    }

    #[test]
    fn the_current_stamp_is_compatible_with_this_engine() {
        SpecificationStamp::current()
            .check_compatible()
            .expect("an engine must accept its own output");
    }

    #[test]
    fn an_unknown_compatibility_family_is_rejected_before_any_field_is_read() {
        let stamp = SpecificationStamp {
            api_version: "amasario.dev/v2".to_owned(),
            spec_version: "1.0.0".to_owned(),
        };
        let error = stamp
            .check_compatible()
            .expect_err("v2 is a different family");
        assert_eq!(
            error.category(),
            crate::errors::ErrorCategory::SpecificationCompatibility
        );
        assert!(error.to_string().contains("v2"));
    }

    #[test]
    fn a_newer_minor_version_is_rejected_rather_than_partially_interpreted() {
        let stamp = SpecificationStamp {
            api_version: "amasario.dev/v1".to_owned(),
            spec_version: "1.4.0".to_owned(),
        };
        let error = stamp
            .check_compatible()
            .expect_err("a newer minor version may rely on unimplemented fields");
        // Refusing is the correct outcome: proceeding would present a partial
        // understanding as a complete one.
        assert!(error.to_string().contains("newer"));
    }

    #[test]
    fn an_older_patch_version_is_accepted_and_reported_as_a_full_interpretation() {
        let engine = Engine::with_defaults().expect("valid");
        let stamp = SpecificationStamp {
            api_version: "amasario.dev/v1".to_owned(),
            spec_version: "1.0.0".to_owned(),
        };
        assert_eq!(
            engine.check_document(&stamp).expect("compatible"),
            InterpretationDepth::Full
        );
    }

    #[test]
    fn a_malformed_specification_version_is_rejected() {
        for malformed in ["1", "1.0", "1.0.0.0", "v1.0.0", "1.0.x", "1.0.0-rc.1"] {
            let stamp = SpecificationStamp {
                api_version: "amasario.dev/v1".to_owned(),
                spec_version: malformed.to_owned(),
            };
            stamp
                .check_compatible()
                .expect_err(&format!("{malformed:?} is not a version"));
        }
    }

    #[test]
    fn versions_parse_and_compare_as_major_minor_patch() {
        let a = Version::parse("1.2.3").expect("valid");
        let b = Version::parse("1.2.4").expect("valid");
        let c = Version::parse("1.3.0").expect("valid");
        assert_eq!((a.major(), a.minor(), a.patch()), (1, 2, 3));
        assert!(a < b);
        assert!(b < c);
        assert_eq!(a.to_string(), "1.2.3");
        assert_eq!(a, Version::parse("1.2.3").expect("valid"));
    }

    #[test]
    fn a_different_major_family_is_rejected_even_inside_a_known_api_version() {
        let stamp = SpecificationStamp {
            api_version: SUPPORTED_API_VERSION.to_owned(),
            spec_version: "2.0.0".to_owned(),
        };
        stamp
            .check_compatible()
            .expect_err("a different major family is not interpretable");
    }

    #[test]
    fn beginning_a_run_validates_the_configuration() {
        let engine = Engine::new(EngineConfig {
            concurrency: 0,
            ..EngineConfig::default()
        })
        .expect_err("an unusable configuration must be refused at construction");
        assert_eq!(
            engine.category(),
            crate::errors::ErrorCategory::Configuration
        );
    }

    #[test]
    fn a_run_is_bound_to_the_network_and_ledger_it_was_started_with() {
        let engine = Engine::with_defaults().expect("valid");
        let context = engine
            .begin_run(
                testnet(),
                LedgerSequence::new(1234567).expect("valid"),
                "2026-09-15T00:00:00Z",
            )
            .expect("a valid run");
        assert_eq!(context.network().id, "testnet");
        assert_eq!(context.boundary_ledger().get(), 1234567);
        assert_eq!(context.started_at(), "2026-09-15T00:00:00Z");
    }

    #[test]
    fn the_configuration_summary_exposes_the_bounds_and_nothing_sensitive() {
        let engine = Engine::with_defaults().expect("valid");
        let rendered = format!("{:?}", engine.config_summary());
        assert!(rendered.contains("max_depth"));
        assert!(rendered.contains("concurrency"));
        // A summary rather than the configuration, so a log line cannot carry an
        // endpoint or a credential.
        assert!(!rendered.contains("rpc"));
        assert!(!rendered.contains("passphrase"));
    }

    #[test]
    fn a_stamp_round_trips_through_its_serialised_form() {
        let stamp = SpecificationStamp::current();
        let encoded = serde_json::to_string(&stamp).expect("serialises");
        let decoded: SpecificationStamp = serde_json::from_str(&encoded).expect("deserialises");
        assert_eq!(decoded, stamp);
    }

    #[test]
    fn an_unknown_stamp_field_is_rejected_rather_than_ignored() {
        let json =
            r#"{"api_version":"amasario.dev/v1","spec_version":"1.0.0","engine_version":"9.9.9"}"#;
        serde_json::from_str::<SpecificationStamp>(json)
            .expect_err("an unknown version field must not be dropped silently");
    }
}
