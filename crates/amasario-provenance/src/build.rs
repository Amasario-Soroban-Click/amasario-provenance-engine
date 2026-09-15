//! Build provenance: what produced an artifact, and whether the record is enough to
//! reproduce it.
//!
//! # Reproducibility is a claim about a record, not about a build
//!
//! [`Reproducibility`] describes what the *record* supports, not what the world
//! contains. A build that was in fact perfectly reproducible, but whose record omits
//! the toolchain, is `NotAttempted` here - because the record is what the engine has,
//! and claiming otherwise would be claiming knowledge the record does not carry. The
//! distinction is the same one the whole engine is built around: what is recorded is
//! not the same as what is true.
//!
//! # Secrets must not be recorded
//!
//! The specification forbids logging or transporting credentials, and a build
//! environment is the most likely place for one to appear: `CARGO_REGISTRY_TOKEN`,
//! `AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`. [`sanitise_environment`] replaces such a
//! value with a redaction marker and records the *name's existence without its
//! value*, which preserves the useful fact - the build ran with a variable of that
//! name set - while making the secret unrecoverable from the record. Dropping the
//! variable entirely would lose the fact that a build depended on it, and keeping it
//! would put a credential in a report that gets committed.
//!
//! The matching is by name pattern and is deliberately broad. A false positive costs
//! a redacted value in a report; a false negative puts a credential in one.

use std::fmt;
use std::str::FromStr;

use amasario_core::{Digest, EngineError, EntityRef, Result};
use serde::{Deserialize, Serialize};

use crate::artifact::ArtifactIdentity;
use crate::errors::ProvenanceFailure;
use crate::source::Revision;

/// The marker a redacted environment value is replaced with.
///
/// A fixed string rather than an empty value, so that a reader can tell "this
/// variable was set to a credential" from "this variable was set to something empty".
pub const REDACTED: &str = "<redacted>";

/// The substring fragments that make an environment variable name secret-bearing.
///
/// Matched case-insensitively against the variable's *name*. Broad on purpose: a
/// false positive costs a redacted value in a report, and a false negative puts a
/// credential in one.
const SECRET_NAME_FRAGMENTS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "APIKEY",
    "API_KEY",
    "PRIVATE_KEY",
    "SECRET_KEY",
    "ACCESS_KEY",
    "AUTH",
    "SESSION",
    "COOKIE",
    "SIGNING",
];

/// The identifier of a compiler or toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
    /// The toolchain's name, such as `rustc` or `soroban-sdk`.
    pub name: String,
    /// The exact version, which is what makes a build reproducible.
    pub version: String,
    /// Where the toolchain came from, when recorded - a channel, a distribution or a
    /// URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Toolchain {
    /// Records a toolchain.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the name or version is empty. A toolchain
    /// named without a version cannot support reproduction, and an empty name names
    /// nothing at all.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let version = version.into();
        if name.is_empty() {
            return Err(EngineError::Validation {
                path: "/build/toolchain/name".to_owned(),
                detail: "a toolchain needs a name".to_owned(),
            });
        }
        if version.is_empty() {
            return Err(EngineError::Validation {
                path: "/build/toolchain/version".to_owned(),
                detail: format!(
                    "toolchain {name} needs a version; without one the build cannot be reproduced \
                     from the record alone"
                ),
            });
        }
        Ok(Self {
            name,
            version,
            source: None,
        })
    }

    /// Records where the toolchain came from.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The toolchain in its canonical `name version` form.
    #[must_use]
    pub fn identity(&self) -> String {
        format!("{} {}", self.name, self.version)
    }
}

impl fmt::Display for Toolchain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}

/// What the record says about reproducing the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReproducibilityStatus {
    /// The build was performed twice and produced the same digest.
    Reproduced,
    /// The build was performed twice and produced different digests.
    ///
    /// A finding about the build, not a defect in the record. It means the artifact
    /// cannot be attributed to the source on the strength of this record alone.
    NotReproduced,
    /// The record does not contain enough to attempt a reproduction.
    NotAttempted,
    /// A reproduction was attempted and its result could not be established.
    Unknown,
}

impl ReproducibilityStatus {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reproduced => "REPRODUCED",
            Self::NotReproduced => "NOT_REPRODUCED",
            Self::NotAttempted => "NOT_ATTEMPTED",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Whether a reproduction was actually carried out and agreed.
    ///
    /// Only `Reproduced`. Every other status, including `NotAttempted`, leaves the
    /// artifact's reproducibility undetermined, and a caller that treated
    /// `NotAttempted` as agreement would report a chain as verified on the strength
    /// of a check nobody performed.
    #[must_use]
    pub const fn is_established(self) -> bool {
        matches!(self, Self::Reproduced)
    }

    /// Whether this status is a finding against the build rather than an absence of
    /// one.
    #[must_use]
    pub const fn is_refutation(self) -> bool {
        matches!(self, Self::NotReproduced)
    }
}

impl fmt::Display for ReproducibilityStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReproducibilityStatus {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "REPRODUCED" => Ok(Self::Reproduced),
            "NOT_REPRODUCED" => Ok(Self::NotReproduced),
            "NOT_ATTEMPTED" => Ok(Self::NotAttempted),
            "UNKNOWN" => Ok(Self::Unknown),
            other => Err(EngineError::Validation {
                path: "/build/reproducibility/status".to_owned(),
                detail: format!("unrecognised reproducibility status {other:?}"),
            }),
        }
    }
}

/// What the record says about reproducing the build, with its evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reproducibility {
    /// The status.
    pub status: ReproducibilityStatus,
    /// What was compared and what was found, when a reproduction was attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Identifiers of the evidence records that support the status.
    ///
    /// Empty is legitimate here, and only here: `NotAttempted` describes the absence
    /// of a reproduction, and demanding evidence of an attempt that did not happen
    /// would be incoherent. A status that *asserts* something - `Reproduced` or
    /// `NotReproduced` - is required to cite evidence, and
    /// [`Reproducibility::validate`] enforces it.
    // `default` is paired with the skip deliberately. Skipping an empty list is
    // only sound when the field can come back: without `default`, a `NotAttempted`
    // record serialises without `evidence` and then fails to deserialise, so a
    // record the engine wrote could not be read back by the engine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

impl Reproducibility {
    /// Records that no reproduction has been attempted.
    #[must_use]
    pub const fn not_attempted() -> Self {
        Self {
            status: ReproducibilityStatus::NotAttempted,
            detail: None,
            evidence: Vec::new(),
        }
    }

    /// Records a completed reproduction.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, because a status that
    /// asserts a comparison was made must name what it compared.
    pub fn reproduced(evidence: Vec<String>, detail: Option<String>) -> Result<Self> {
        Self::asserting(ReproducibilityStatus::Reproduced, detail, evidence)
    }

    /// Records a repetition that disagreed.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited.
    pub fn not_reproduced(evidence: Vec<String>, detail: Option<String>) -> Result<Self> {
        Self::asserting(ReproducibilityStatus::NotReproduced, detail, evidence)
    }

    /// Builds a status that asserts something, requiring evidence.
    fn asserting(
        status: ReproducibilityStatus,
        detail: Option<String>,
        evidence: Vec<String>,
    ) -> Result<Self> {
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/build/reproducibility/evidence".to_owned(),
                detail: format!(
                    "a {status} status asserts that a comparison was made, so it must cite the \
                     evidence of that comparison"
                ),
            });
        }
        Ok(Self {
            status,
            detail,
            evidence,
        })
    }

    /// Whether this status asserts that a comparison was made.
    #[must_use]
    pub const fn asserts_a_comparison(&self) -> bool {
        !matches!(
            self.status,
            ReproducibilityStatus::NotAttempted | ReproducibilityStatus::Unknown
        )
    }

    /// Checks that a status asserting a comparison cites evidence.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the status asserts something and cites
    /// nothing.
    pub fn validate(&self) -> Result<()> {
        if self.asserts_a_comparison() && self.evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/build/reproducibility/evidence".to_owned(),
                detail: format!(
                    "a {} status asserts that a comparison was made, so it must cite the evidence \
                     of that comparison",
                    self.status
                ),
            });
        }
        Ok(())
    }
}

/// One environment variable recorded with a build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentVariable {
    /// The variable's name.
    pub name: String,
    /// The variable's value, or `None` when it was redacted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Whether the value was withheld.
    ///
    /// Recorded separately from an absent value so that "this variable was set and
    /// its value is withheld" is distinguishable from "this variable was set to
    /// nothing".
    pub redacted: bool,
}

/// Whether a variable's name makes its value secret-bearing.
#[must_use]
pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NAME_FRAGMENTS
        .iter()
        .any(|fragment| upper.contains(fragment))
}

/// Records an environment, redacting secret-bearing values.
///
/// The variable's name and its presence are kept; its value is not. That preserves
/// the fact a build depended on a variable of that name - which is what a
/// reproducibility investigation needs - while making the credential unrecoverable
/// from the record. A build environment can contain a registry token, and a report
/// containing one would put it wherever the report goes.
///
/// The result is sorted by name so that two records of the same environment serialise
/// identically, which matters because the environment participates in the build
/// record's digest.
#[must_use]
pub fn sanitise_environment<I, K, V>(environment: I) -> Vec<EnvironmentVariable>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let mut recorded: Vec<EnvironmentVariable> = environment
        .into_iter()
        .map(|(name, value)| {
            let name = name.into();
            if is_secret_name(&name) {
                EnvironmentVariable {
                    name,
                    value: None,
                    redacted: true,
                }
            } else {
                EnvironmentVariable {
                    name,
                    value: Some(value.into()),
                    redacted: false,
                }
            }
        })
        .collect();
    recorded.sort_by(|a, b| a.name.cmp(&b.name));
    recorded
}

/// What produced an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildProvenance {
    /// The source revision the build read.
    pub source_revision: Revision,
    /// The compiler or toolchain that ran.
    pub toolchain: Toolchain,
    /// The build target, such as `wasm32-unknown-unknown`.
    pub target: String,
    /// The digest of the build's configuration, when recorded.
    ///
    /// A digest rather than the configuration itself: the configuration can contain
    /// paths and credentials, and what a consumer needs is whether two builds used
    /// the same one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_digest: Option<Digest>,
    /// The digest of the dependency lockfile the build resolved against.
    ///
    /// Recorded because two builds of the same source revision with the same
    /// toolchain can still differ if their dependency resolution differed, which is
    /// the case a build-provenance record exists to catch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lockfile_digest: Option<Digest>,
    /// The artifact the build produced.
    pub artifact: ArtifactIdentity,
    /// What the record says about reproducing the build.
    pub reproducibility: Reproducibility,
    /// The build environment, with secret-bearing values redacted.
    // `default` is paired with the skip for the same reason as on
    // `Reproducibility::evidence`: a record without a recorded environment must
    // still round-trip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<EnvironmentVariable>,
    /// Identifiers of the evidence records that support this claim.
    pub evidence: Vec<String>,
}

impl BuildProvenance {
    /// Records a build.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, and a provenance error
    /// when the artifact is not one a build produces.
    pub fn new(
        source_revision: Revision,
        toolchain: Toolchain,
        target: impl Into<String>,
        artifact: ArtifactIdentity,
        evidence: Vec<String>,
    ) -> Result<Self> {
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/buildProvenance/evidence".to_owned(),
                detail: "a build claim must cite the evidence that supports it".to_owned(),
            });
        }
        if !artifact.artifact_type.is_build_output() {
            return Err(ProvenanceFailure::BuildUnrecorded {
                artifact: artifact.digest.prefixed(),
            }
            .into_error());
        }
        Ok(Self {
            source_revision,
            toolchain,
            target: target.into(),
            configuration_digest: None,
            lockfile_digest: None,
            artifact,
            reproducibility: Reproducibility::not_attempted(),
            environment: Vec::new(),
            evidence,
        })
    }

    /// Records the configuration digest.
    #[must_use]
    pub fn with_configuration_digest(mut self, digest: Digest) -> Self {
        self.configuration_digest = Some(digest);
        self
    }

    /// Records the lockfile digest.
    #[must_use]
    pub fn with_lockfile_digest(mut self, digest: Digest) -> Self {
        self.lockfile_digest = Some(digest);
        self
    }

    /// Records what the record says about reproduction.
    #[must_use]
    pub fn with_reproducibility(mut self, reproducibility: Reproducibility) -> Self {
        self.reproducibility = reproducibility;
        self
    }

    /// Records the build environment, redacting secret-bearing values.
    ///
    /// Takes the environment rather than accepting already-sanitised variables, so
    /// that a caller cannot forget to sanitise.
    #[must_use]
    pub fn with_environment<I, K, V>(mut self, environment: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.environment = sanitise_environment(environment);
        self
    }

    /// A reference to the build as an entity.
    ///
    /// A build is identified by a digest over the facts that determine its output:
    /// the source revision, the toolchain, the target, the configuration and the
    /// lockfile. Two builds that agree on all of those should produce the same
    /// artifact, which is exactly what makes that set the identity.
    #[must_use]
    pub fn as_ref(&self) -> EntityRef {
        EntityRef {
            kind: amasario_core::EntityKind::Build,
            id: self.identity_digest().value().to_owned(),
        }
    }

    /// A digest over the facts that determine what a build produces.
    ///
    /// Deliberately excludes the artifact, the reproducibility result, the
    /// environment and the evidence: those describe what *this* build did, while the
    /// identity describes which build it was. Including the artifact would make the
    /// identity depend on the outcome, so a build that produced a different artifact
    /// would look like a different build rather than a non-reproducible one - which
    /// is the finding that matters.
    #[must_use]
    pub fn identity_digest(&self) -> Digest {
        let mut canonical = String::with_capacity(256);
        canonical.push_str("amasario/build\n");
        canonical.push_str(self.source_revision.kind.as_str());
        canonical.push('\n');
        canonical.push_str(&self.source_revision.value);
        canonical.push('\n');
        canonical.push_str(
            self.source_revision
                .resolved_commit
                .as_deref()
                .unwrap_or("-"),
        );
        canonical.push('\n');
        canonical.push_str(&self.toolchain.identity());
        canonical.push('\n');
        canonical.push_str(&self.target);
        canonical.push('\n');
        canonical.push_str(
            self.configuration_digest
                .as_ref()
                .map_or("-", Digest::value),
        );
        canonical.push('\n');
        canonical.push_str(self.lockfile_digest.as_ref().map_or("-", Digest::value));
        canonical.push('\n');
        Digest::sha256_of(canonical.as_bytes())
    }

    /// Whether two records describe the same build.
    #[must_use]
    pub fn is_same_build_as(&self, other: &Self) -> bool {
        self.identity_digest().matches(&other.identity_digest())
    }

    /// Whether the record is sufficient to attempt a reproduction.
    ///
    /// Requires a revision that resolves to a commit and a toolchain with a version,
    /// both of which the constructors enforce; the method exists so that the question
    /// has one answer in one place rather than being re-derived at a report site.
    #[must_use]
    pub const fn is_reproducible_from_record(&self) -> bool {
        self.source_revision.is_immutable()
    }

    /// Checks the record's own invariants.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, when the artifact is not
    /// a build output, or when the reproducibility status asserts a comparison
    /// without citing one.
    pub fn validate(&self) -> Result<()> {
        if self.evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/buildProvenance/evidence".to_owned(),
                detail: "a build claim must cite the evidence that supports it".to_owned(),
            });
        }
        self.artifact.validate()?;
        self.reproducibility.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::ArtifactType;
    use crate::source::RevisionKind;

    const SHA1: &str = "9f2c1e0a3b4c5d6e7f8091a2b3c4d5e6f7081920";

    fn artifact() -> ArtifactIdentity {
        ArtifactIdentity::new(
            Digest::sha256_of(b"module"),
            ArtifactType::Wasm,
            vec!["e".to_owned()],
        )
        .expect("a valid artifact")
    }

    fn build() -> BuildProvenance {
        BuildProvenance::new(
            Revision::commit(SHA1).expect("a valid commit"),
            Toolchain::new("rustc", "1.93.0").expect("a valid toolchain"),
            "wasm32-unknown-unknown",
            artifact(),
            vec!["e".to_owned()],
        )
        .expect("a valid build")
    }

    #[test]
    fn a_toolchain_needs_both_a_name_and_a_version() {
        // Without a version the build cannot be reproduced from the record, which is
        // the whole purpose of recording a toolchain.
        Toolchain::new("rustc", "1.93.0").expect("a complete toolchain");
        let error = Toolchain::new("rustc", "").expect_err("no version");
        assert!(
            error
                .to_string()
                .contains("cannot be reproduced from the record alone")
        );
        Toolchain::new("", "1.93.0").expect_err("no name names nothing");
        assert_eq!(
            Toolchain::new("rustc", "1.93.0").expect("valid").identity(),
            "rustc 1.93.0"
        );
    }

    #[test]
    fn a_build_must_cite_evidence_and_produce_a_build_output() {
        BuildProvenance::new(
            Revision::commit(SHA1).expect("valid"),
            Toolchain::new("rustc", "1").expect("valid"),
            "wasm32-unknown-unknown",
            artifact(),
            Vec::new(),
        )
        .expect_err("no evidence");

        let archive = ArtifactIdentity::new(
            Digest::sha256_of(b"tar"),
            ArtifactType::SourceArchive,
            vec!["e".to_owned()],
        )
        .expect("a valid artifact");
        BuildProvenance::new(
            Revision::commit(SHA1).expect("valid"),
            Toolchain::new("rustc", "1").expect("valid"),
            "wasm32-unknown-unknown",
            archive,
            vec!["e".to_owned()],
        )
        .expect_err("a build does not produce a source archive");
    }

    #[test]
    fn the_build_identity_excludes_the_artifact_and_the_outcome() {
        // Including the artifact would make the identity depend on the outcome, so a
        // build that produced a different artifact would look like a different build
        // rather than a non-reproducible one - which is the finding that matters.
        let first = build();
        let mut second = build();
        second.artifact = ArtifactIdentity::new(
            Digest::sha256_of(b"something else"),
            ArtifactType::Wasm,
            vec!["e".to_owned()],
        )
        .expect("a valid artifact");
        second.reproducibility =
            Reproducibility::not_reproduced(vec!["e".to_owned()], None).expect("valid");
        second.evidence.push("another".to_owned());

        assert!(
            first.is_same_build_as(&second),
            "the same inputs are the same build whatever they produced"
        );
        assert_eq!(first.as_ref().kind, amasario_core::EntityKind::Build);
    }

    #[test]
    fn the_build_identity_distinguishes_each_determining_fact() {
        let base = build();
        let other_revision = BuildProvenance::new(
            Revision::commit("a".repeat(40)).expect("valid"),
            Toolchain::new("rustc", "1.93.0").expect("valid"),
            "wasm32-unknown-unknown",
            artifact(),
            vec!["e".to_owned()],
        )
        .expect("valid");
        assert!(!base.is_same_build_as(&other_revision));

        let other_toolchain = BuildProvenance::new(
            Revision::commit(SHA1).expect("valid"),
            Toolchain::new("rustc", "1.94.0").expect("valid"),
            "wasm32-unknown-unknown",
            artifact(),
            vec!["e".to_owned()],
        )
        .expect("valid");
        assert!(!base.is_same_build_as(&other_toolchain));

        let other_target = BuildProvenance::new(
            Revision::commit(SHA1).expect("valid"),
            Toolchain::new("rustc", "1.93.0").expect("valid"),
            "wasm32v1-none",
            artifact(),
            vec!["e".to_owned()],
        )
        .expect("valid");
        assert!(!base.is_same_build_as(&other_target));

        let with_configuration = base
            .clone()
            .with_configuration_digest(Digest::sha256_of(b"cfg"));
        assert!(!base.is_same_build_as(&with_configuration));

        let with_lockfile = base
            .clone()
            .with_lockfile_digest(Digest::sha256_of(b"lock"));
        assert!(!base.is_same_build_as(&with_lockfile));
        assert_ne!(
            with_configuration.identity_digest(),
            with_lockfile.identity_digest(),
            "the configuration and the lockfile must not be conflated"
        );
    }

    #[test]
    fn a_local_variable_and_a_secret_shaped_name_are_both_recognised() {
        assert!(is_secret_name("CARGO_REGISTRY_TOKEN"));
        assert!(is_secret_name("github_token"));
        assert!(is_secret_name("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_name("DB_PASSWORD"));
        assert!(is_secret_name("MY_API_KEY"));
        assert!(is_secret_name("SSH_AUTH_SOCK"));
        assert!(!is_secret_name("PATH"));
        assert!(!is_secret_name("CARGO_HOME"));
        assert!(!is_secret_name("RUSTFLAGS"));
    }

    #[test]
    fn a_secret_bearing_variable_keeps_its_name_and_loses_its_value() {
        // The name's presence is what a reproducibility investigation needs; the
        // value is what must not reach a report that gets committed.
        let environment = sanitise_environment([
            ("PATH", "/usr/bin"),
            ("CARGO_REGISTRY_TOKEN", "crates-io-super-secret"),
        ]);

        assert_eq!(environment.len(), 2);
        // Sorted by name, so `CARGO_REGISTRY_TOKEN` precedes `PATH`.
        assert_eq!(environment[0].name, "CARGO_REGISTRY_TOKEN");
        assert!(environment[0].redacted);
        assert_eq!(environment[0].value, None);
        assert_eq!(environment[1].name, "PATH");
        assert!(!environment[1].redacted);
        assert_eq!(environment[1].value.as_deref(), Some("/usr/bin"));

        let serialised = serde_json::to_string(&environment).expect("serialises");
        assert!(
            !serialised.contains("crates-io-super-secret"),
            "the credential must not survive serialisation: {serialised}"
        );
    }

    #[test]
    fn an_empty_value_is_distinguishable_from_a_redacted_one() {
        let environment = sanitise_environment([("RUSTFLAGS", ""), ("MY_TOKEN", "")]);
        let rustflags = environment
            .iter()
            .find(|variable| variable.name == "RUSTFLAGS")
            .expect("present");
        let token = environment
            .iter()
            .find(|variable| variable.name == "MY_TOKEN")
            .expect("present");

        assert_eq!(rustflags.value.as_deref(), Some(""));
        assert!(!rustflags.redacted);
        assert_eq!(token.value, None);
        assert!(token.redacted);
    }

    #[test]
    fn a_build_records_its_sanitised_environment() {
        let build = build().with_environment([("CARGO_REGISTRY_TOKEN", "secret-value")]);
        assert_eq!(build.environment.len(), 1);
        assert!(build.environment[0].redacted);
        let serialised = serde_json::to_string(&build).expect("serialises");
        assert!(!serialised.contains("secret-value"));
    }

    #[test]
    fn a_status_that_asserts_a_comparison_must_cite_evidence() {
        Reproducibility::not_attempted()
            .validate()
            .expect("not attempting a comparison asserts nothing");

        let error = Reproducibility::reproduced(Vec::new(), None)
            .expect_err("a reproduction must cite what it compared");
        assert!(error.to_string().contains("must cite the evidence"));

        Reproducibility::reproduced(vec!["e".to_owned()], Some("same digest".to_owned()))
            .expect("valid")
            .validate()
            .expect("self-consistent");
        Reproducibility::not_reproduced(vec!["e".to_owned()], None).expect("valid");
    }

    #[test]
    fn only_a_completed_reproduction_establishes_reproducibility() {
        // A caller that treated NotAttempted as agreement would report a chain as
        // verified on the strength of a check nobody performed.
        assert!(ReproducibilityStatus::Reproduced.is_established());
        assert!(!ReproducibilityStatus::NotAttempted.is_established());
        assert!(!ReproducibilityStatus::Unknown.is_established());
        assert!(!ReproducibilityStatus::NotReproduced.is_established());
        assert!(ReproducibilityStatus::NotReproduced.is_refutation());
    }

    #[test]
    fn reproducibility_statuses_round_trip_through_their_wire_names() {
        for status in [
            ReproducibilityStatus::Reproduced,
            ReproducibilityStatus::NotReproduced,
            ReproducibilityStatus::NotAttempted,
            ReproducibilityStatus::Unknown,
        ] {
            assert_eq!(
                ReproducibilityStatus::from_str(status.as_str()).expect("round trip"),
                status
            );
        }
        ReproducibilityStatus::from_str("MAYBE").expect_err("an unknown status is rejected");
    }

    #[test]
    fn a_build_from_a_branch_revision_is_not_reproducible_from_the_record() {
        let build = BuildProvenance::new(
            Revision::branch("main").expect("valid"),
            Toolchain::new("rustc", "1.93.0").expect("valid"),
            "wasm32-unknown-unknown",
            artifact(),
            vec!["e".to_owned()],
        )
        .expect("valid");
        assert!(!build.is_reproducible_from_record());
        assert_eq!(build.source_revision.kind, RevisionKind::Branch);

        let resolved = BuildProvenance::new(
            Revision::branch("main")
                .expect("valid")
                .resolved_to(SHA1)
                .expect("valid"),
            Toolchain::new("rustc", "1.93.0").expect("valid"),
            "wasm32-unknown-unknown",
            artifact(),
            vec!["e".to_owned()],
        )
        .expect("valid");
        assert!(resolved.is_reproducible_from_record());
    }

    #[test]
    fn a_deserialised_build_is_caught_by_validation() {
        let mut build = build();
        build.validate().expect("self-consistent");
        build.evidence.clear();
        build.validate().expect_err("not a record without evidence");
    }

    #[test]
    fn a_build_round_trips_through_json() {
        let original = build()
            .with_configuration_digest(Digest::sha256_of(b"cfg"))
            .with_lockfile_digest(Digest::sha256_of(b"lock"))
            .with_environment([("PATH", "/usr/bin")]);
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: BuildProvenance = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, original);
        restored.validate().expect("self-consistent");
    }
}
