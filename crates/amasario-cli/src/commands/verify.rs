//! `amasario verify` - a gate a release pipeline can branch on.
//!
//! # What is being verified
//!
//! Verification answers one question: does the evidence support the claim. Here the claim
//! is the executable's identity, and the evidence is the digest the network reported
//! compared against the bytes actually retrieved. The outcomes are the specification's
//! five statuses, `VERIFIED`, `PARTIALLY_VERIFIED`, `UNVERIFIED`, `CONFLICTING` and
//! `UNKNOWN`, and they are not degrees of one scale. A chain that is `CONFLICTING` is not a
//! verified chain; it is a chain whose evidence refutes a claim, and the command exits
//! non-zero for it without being asked.
//!
//! # `UNKNOWN` is not a failure
//!
//! A Stellar Asset Contract has no module, so there is nothing to verify. Reporting that as
//! a failure would make every asset contract fail a provenance gate, which would train a
//! reader to ignore the gate. `UNKNOWN` therefore passes unless `--require` names a level
//! the outcome does not reach.

use amasario_core::VerificationStatus;
use clap::{Args, ValueEnum};
use serde_json::{Value, json};

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::{observe, to_value};
use crate::commands::provenance::parse_digest;

/// The verification-specific arguments.
#[derive(Debug, Clone, Args)]
pub struct VerifyArgs {
    /// A claimed SHA-256 executable digest to compare against what the network reports.
    #[arg(long = "claimed-digest", value_name = "HEX")]
    pub claimed_digest: Option<String>,

    /// Require at least this outcome, failing the gate otherwise.
    #[arg(long, value_enum, value_name = "STATUS")]
    pub require: Option<RequiredStatus>,
}

/// The minimum outcome a run may accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RequiredStatus {
    /// Every component checked out.
    Verified,
    /// Some components checked out and none was contradicted.
    Partially,
    /// Nothing could be checked but nothing was contradicted.
    Unverified,
}

impl RequiredStatus {
    /// The status this requires.
    #[must_use]
    const fn status(self) -> VerificationStatus {
        match self {
            Self::Verified => VerificationStatus::Verified,
            Self::Partially => VerificationStatus::PartiallyVerified,
            Self::Unverified => VerificationStatus::Unverified,
        }
    }

    /// Whether an outcome satisfies the requirement.
    ///
    /// A contradiction never satisfies any requirement, which is why it is checked first
    /// rather than ranked: `CONFLICTING` is not below `VERIFIED`, it is a different thing.
    #[must_use]
    const fn satisfied_by(self, status: VerificationStatus) -> bool {
        if status.is_refutation() {
            return false;
        }
        rank(status) >= rank(self.status())
    }
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a usage error for a malformed claimed digest, and a
/// gate failure when the outcome is contradicted or short of `--require`.
pub async fn run(
    target: TargetArgs,
    bounds: BoundsArgs,
    verify: VerifyArgs,
    output: OutputArgs,
) -> CliResult<()> {
    let inspection = observe(&target, &bounds).await?;

    let claimed = verify
        .claimed_digest
        .as_deref()
        .map(parse_digest)
        .transpose()?;

    let recorded = inspection.identity.wasm_hash.clone();

    // The status of the observed executable's identity.
    let (status, reason) = match (&recorded, inspection.digest_verified) {
        (None, _) => (
            VerificationStatus::Unknown,
            "the contract executes no module, so there is no executable identity to verify"
                .to_owned(),
        ),
        (Some(_), Some(true)) => (VerificationStatus::Verified, VERIFIED_REASON.to_owned()),
        (Some(_), Some(false)) => (
            VerificationStatus::Conflicting,
            "the module bytes the endpoint served do not hash to the executable digest the \
             same endpoint records; the endpoint's own two readings disagree, which is a \
             contradiction rather than a weak verification"
                .to_owned(),
        ),
        (Some(_), None) => (
            VerificationStatus::Unverified,
            "the executable digest was recorded but the module's bytes were not retrieved, \
             so nothing could be checked"
                .to_owned(),
        ),
    };

    // A claimed digest refines the outcome: it is a second claim, and a mismatch is a
    // contradiction even when the module itself verified.
    let (status, reason) = match (&claimed, &recorded) {
        (Some(claimed_digest), Some(reported)) => {
            if claimed_digest == reported {
                (
                    status,
                    format!("{reason}; the claimed digest matches the recorded one"),
                )
            } else {
                (
                    VerificationStatus::Conflicting,
                    format!(
                        "{reason}; the claimed digest {} does not match the recorded {}",
                        claimed_digest.value(),
                        reported.value()
                    ),
                )
            }
        },
        (Some(_), None) => (
            VerificationStatus::Unverified,
            "a digest was claimed, but the contract reports no executable digest to compare it \
             with"
                .to_owned(),
        ),
        (None, _) => (status, reason),
    };

    let rendered = match output.format {
        OutputFormat::Json => {
            let value = view(&target.contract, &inspection, status, &reason, &verify)?;
            if output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&target.contract, status, &reason),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "verify renders text or json, not {}; use `report` for Markdown and JUnit",
                output.format.as_str()
            )));
        },
    };

    emit(&rendered, output.output.as_deref())?;

    if status.is_refutation() {
        return Err(CliError::Gate(format!(
            "verification of {} is {}: {reason}",
            target.contract,
            status.as_str()
        )));
    }
    if let Some(required) = verify.require
        && !required.satisfied_by(status)
    {
        return Err(CliError::Gate(format!(
            "verification of {} is {}, which is short of the required {}",
            target.contract,
            status.as_str(),
            required.status().as_str()
        )));
    }

    Ok(())
}

/// The published JSON view.
fn view(
    contract: &str,
    inspection: &amasario_contract::ContractInspection,
    status: VerificationStatus,
    reason: &str,
    verify: &VerifyArgs,
) -> CliResult<Value> {
    let recorded = inspection
        .identity
        .wasm_hash
        .as_ref()
        .map(|digest| digest.value().to_owned());

    Ok(json!({
        "contract": contract,
        "network": inspection.identity.network_id,
        "status": status.as_str(),
        "reason": reason,
        "recordedDigest": recorded,
        "moduleDigestVerified": inspection.digest_verified,
        "claimedDigest": verify.claimed_digest,
        "required": verify.require.map(|required| required.status().as_str().to_owned()),
        // What the status is a statement about. Emitted as part of the document rather
        // than left to the documentation, because a gate reads this field and not the
        // prose: a pipeline that branched on a bare `VERIFIED` would be reading a claim
        // narrower than the word suggests, and the narrowing belongs where it is read.
        "scope": scope(),
        "boundary": to_value(&inspection.boundary)?,
        "anomalies": inspection
            .anomalies
            .iter()
            .map(|failure| amasario_contract::describe(failure).to_owned())
            .collect::<Vec<String>>(),
    }))
}

/// The reason reported with a `VERIFIED` executable identity.
///
/// # Why the sentence is this specific
///
/// The comparison is between two readings of one source: the module bytes the endpoint
/// served, and the executable digest the same endpoint reports. It is a real check - an
/// endpoint that contradicts itself is a finding - but `VERIFIED` is a strong word for
/// it, and a reader who takes it for an independent attestation has been misled by one
/// word rather than by a wrong answer. So the reason names both sides as coming from the
/// endpoint and says what that does and does not establish, and the test below pins it
/// because the wording is the whole of the qualification.
const VERIFIED_REASON: &str = "the module bytes the endpoint served hash to the executable digest the same endpoint \
     records; both readings come from the endpoint, so this establishes that the response \
     is internally consistent, not that any other party corroborates it";

/// What `verify` checked, and what it did not.
///
/// # Why the boundary is spelled out
///
/// The strongest thing this command can establish is a consistency check within one
/// source: the module bytes the endpoint served against the executable digest the same
/// endpoint reports. That is worth having - an endpoint contradicting itself is a real
/// finding, and a `CONFLICTING` result is a refutation - but `VERIFIED` is a strong word
/// for it, and a reader who takes it for an independent attestation has been misled by a
/// single word rather than by a wrong answer.
///
/// So the two lists travel with the status in every rendering: the readings that were
/// compared, and the questions this command does not ask. A consumer that needs one of
/// the unchecked properties has to obtain it from somewhere else, and the document says
/// which properties those are instead of leaving them to be inferred.
fn scope() -> Value {
    json!({
        "checked": [
            "the module bytes the endpoint served, against the executable digest the same \
             endpoint reports for the contract",
        ],
        "notChecked": [
            "that the recorded executable digest corresponds to any source revision or \
             build, which is the provenance analysis rather than this command",
            "that any party other than the endpoint corroborates the recorded digest",
            "that the contract is secure, safe, correct or free of vulnerabilities",
        ],
    })
}

/// The human-readable view.
fn text(contract: &str, status: VerificationStatus, reason: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "verify      {contract}");
    let _ = writeln!(out, "status      {}", status.as_str());
    let _ = writeln!(out, "reason      {reason}");
    let _ = writeln!(
        out,
        "checked     the module bytes the endpoint served against the digest the same \
         endpoint reports"
    );
    let _ = writeln!(
        out,
        "not checked source or build correspondence, corroboration by any other party, \
         and anything about safety"
    );
    let _ = writeln!(out);
    let _ = write!(
        out,
        "VERIFIED states that the evidence is consistent with the identity claim, not that \
         the contract is safe or trustworthy. Amasario is not a security scanner."
    );
    out
}

/// The strength of an outcome that is not a refutation.
///
/// Stated here rather than taken from the type, because the specification orders the
/// statuses for combination and does not rank `CONFLICTING` among them: a contradiction
/// is not below `VERIFIED`, it is a different kind of answer. The refutation is therefore
/// checked before this is consulted, and this function never has to place it.
const fn rank(status: VerificationStatus) -> u8 {
    match status {
        VerificationStatus::Verified => 3,
        VerificationStatus::PartiallyVerified => 2,
        VerificationStatus::Unverified => 1,
        // `CONFLICTING` is checked before this is consulted, and a status from a later
        // specification version is ranked below `UNVERIFIED` because this engine cannot
        // say what it establishes.
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contradiction_never_satisfies_a_requirement() {
        for required in [
            RequiredStatus::Verified,
            RequiredStatus::Partially,
            RequiredStatus::Unverified,
        ] {
            assert!(
                !required.satisfied_by(VerificationStatus::Conflicting),
                "{:?} accepted a contradiction",
                required
            );
        }
    }

    #[test]
    fn a_requirement_is_satisfied_by_a_stronger_or_equal_outcome() {
        assert!(RequiredStatus::Unverified.satisfied_by(VerificationStatus::Verified));
        assert!(RequiredStatus::Partially.satisfied_by(VerificationStatus::Verified));
        assert!(RequiredStatus::Verified.satisfied_by(VerificationStatus::Verified));
    }

    #[test]
    fn a_requirement_is_not_satisfied_by_a_weaker_outcome() {
        assert!(!RequiredStatus::Verified.satisfied_by(VerificationStatus::PartiallyVerified));
        assert!(!RequiredStatus::Partially.satisfied_by(VerificationStatus::Unverified));
        assert!(!RequiredStatus::Verified.satisfied_by(VerificationStatus::Unknown));
    }

    #[test]
    fn the_scope_names_what_was_not_checked_as_well_as_what_was() {
        // The point of the field is the negative half. A consumer reading only `checked`
        // would take `VERIFIED` for an independent attestation, which is the misreading
        // this exists to prevent, so both lists must be present and non-empty and the
        // unchecked one must name the three things a reader would otherwise assume.
        let scope = scope();
        let checked = scope["checked"].as_array().expect("a checked list");
        let unchecked = scope["notChecked"].as_array().expect("an unchecked list");

        assert_eq!(checked.len(), 1, "one comparison is made");
        assert!(
            unchecked.len() >= 3,
            "the unchecked list must be the longer one: this command checks one thing"
        );

        let joined = unchecked
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        for absent in ["source", "corroborates", "secure"] {
            assert!(
                joined.contains(absent),
                "the unchecked list must say that `{absent}` was not established: {joined}"
            );
        }
    }

    #[test]
    fn the_verified_reason_says_both_readings_came_from_the_endpoint() {
        // Pinned because it is the sentence that stops `VERIFIED` reading as an
        // independent attestation. If it ever loses the same-source clause it has become
        // a different claim than the one the command makes, and the word `VERIFIED`
        // would then be the only thing left saying so.
        assert!(
            VERIFIED_REASON.contains("both readings come from the endpoint"),
            "the reason must say the comparison is within one source: {VERIFIED_REASON}"
        );
        assert!(
            VERIFIED_REASON.contains("internally consistent"),
            "the reason must name what consistency is established: {VERIFIED_REASON}"
        );
        assert!(
            VERIFIED_REASON.contains("not"),
            "the reason must state the limit of the claim: {VERIFIED_REASON}"
        );
    }
}
