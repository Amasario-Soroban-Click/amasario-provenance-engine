//! `amasario provenance` - where the deployed executable came from, and how well that is
//! established.
//!
//! # What this command can and cannot establish
//!
//! The engine observes three things without help: the address, the executable digest the
//! network reports, and - when the endpoint resolved it - the operation that last modified
//! the contract's instance entry. From those it can establish that the module's bytes hash
//! to the recorded digest, and that a deployment record placed the contract.
//!
//! Everything before the module - the source revision, the build, the artifact - is a
//! *claim* until it is checked. The command accepts a claimed artifact digest with
//! `--artifact-digest` and compares it with the digest the network reports, which is the
//! one comparison that can turn a claim into a contradiction. A claimed source revision is
//! reported as unverified rather than folded into the chain, because the engine has no
//! evidence for it and a chain link without evidence would be a fabricated one.
//!
//! # The statuses are not interchangeable
//!
//! `CONFLICTING` means the evidence refutes a claim, and it wins over everything. It is
//! not a synonym for `UNVERIFIED`, which means nothing was checked, and neither is a
//! synonym for `UNKNOWN`, which means the question could not be evaluated at all. The
//! command exits with a non-zero status only for `CONFLICTING`, because that is the one
//! outcome a release gate must stop on.

use amasario_core::{
    Basis, Confidence, ConfidenceLevel, Digest, DigestAlgorithm, EntityKind, EntityRef,
};
use amasario_evidence::artifact::{compare_executable, from_executable};
use amasario_evidence::collector::EvidenceRecord;
use amasario_provenance::{
    ChainLink, ChainLinkKind, ProvenanceChain, VerificationOutcome, verify_chain,
};
use clap::Args;
use serde_json::{Value, json};

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::{observe, to_value};

/// The provenance-specific arguments.
#[derive(Debug, Clone, Args)]
pub struct ProvenanceArgs {
    /// A claimed SHA-256 digest of the build artifact the contract was deployed from.
    #[arg(long = "artifact-digest", value_name = "HEX")]
    pub artifact_digest: Option<String>,

    /// A claimed source repository. Reported as unverified: the engine holds no evidence
    /// for it.
    #[arg(long = "source-repo", value_name = "URL")]
    pub source_repo: Option<String>,

    /// A claimed source revision. Reported as unverified.
    #[arg(long = "revision", value_name = "REV")]
    pub revision: Option<String>,
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a validation error when a supplied digest is not a
/// SHA-256 hex value, and a gate failure when the chain is contradicted.
pub async fn run(
    target: TargetArgs,
    bounds: BoundsArgs,
    provenance: ProvenanceArgs,
    output: OutputArgs,
) -> CliResult<()> {
    let inspection = observe(&target, &bounds).await?;
    let contract = inspection.identity.contract_id.clone();
    let recorded = inspection.identity.wasm_hash.clone();

    let claimed = provenance
        .artifact_digest
        .as_deref()
        .map(parse_digest)
        .transpose()?;

    // The record for what the network reported. It exists whenever a module was observed,
    // because it is the observation the whole chain rests on.
    let observation_id = format!("evidence:wasm:{contract}");
    let observation = recorded
        .as_ref()
        .map(|digest| {
            from_executable(
                digest.clone(),
                &contract,
                inspection.boundary.clone(),
                observation_id.clone(),
                inspection.boundary.observed_at.clone(),
            )
        })
        .transpose()?;

    let mut chain =
        ProvenanceChain::new(EntityRef::new(EntityKind::Contract, contract.to_string())?)?;

    // The one comparison that can turn a claim into a contradiction.
    let comparison = match (&claimed, &recorded) {
        (Some(claimed_digest), Some(reported)) => {
            let record = compare_executable(
                claimed_digest,
                reported,
                &contract,
                inspection.boundary.clone(),
                format!("evidence:artifact:{contract}"),
                inspection.boundary.observed_at.clone(),
            )?;
            let matched = claimed_digest == reported;
            let evidence = vec![record.id.clone()];
            let contradicting = if matched {
                Vec::new()
            } else {
                // The contradicting citation is the record itself, which states the
                // comparison it made.
                vec![record.id.clone()]
            };
            let confidence = Confidence::new(
                if matched {
                    ConfidenceLevel::HighConfidence
                } else {
                    ConfidenceLevel::Verified
                },
                evidence,
                contradicting,
            )?;
            chain.push(ChainLink::new(
                ChainLinkKind::ArtifactToWasm,
                EntityRef::new(EntityKind::Wasm, reported.value())?,
                Some(EntityRef::new(
                    EntityKind::Artifact,
                    claimed_digest.value(),
                )?),
                Basis::EmbeddedDigest,
                confidence,
                if matched {
                    amasario_core::VerificationStatus::Verified
                } else {
                    amasario_core::VerificationStatus::Conflicting
                },
            )?)?;
            Some(record)
        },
        _ => None,
    };

    // When the endpoint resolved the operation that last modified the instance entry, the
    // deployment is an established fact rather than a claim.
    if let Some(modification) = &inspection.identity.instance_modification
        && let Some(transaction) = &modification.transaction
    {
        let deployment_id = format!("{transaction}@{}", modification.ledger);
        let evidence = vec![format!("evidence:deployment:{deployment_id}")];
        chain.push(ChainLink::new(
            ChainLinkKind::DeploymentToContract,
            EntityRef::new(EntityKind::Contract, contract.to_string())?,
            Some(EntityRef::new(
                EntityKind::Deployment,
                deployment_id.as_str(),
            )?),
            Basis::ObservedInvocation,
            Confidence::new(ConfidenceLevel::HighConfidence, evidence, Vec::new())?,
            amasario_core::VerificationStatus::Verified,
        )?)?;
    }

    let outcome = verify_chain(&chain);

    let rendered = match output.format {
        OutputFormat::Json => {
            let value = view(
                &inspection,
                &outcome,
                observation.as_ref(),
                comparison.as_ref(),
                &provenance,
                &chain,
            )?;
            if output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&outcome, &provenance, &chain, &inspection),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "provenance renders text or json, not {}",
                output.format.as_str()
            )));
        },
    };

    emit(&rendered, output.output.as_deref())?;

    if outcome.is_contradicted() {
        return Err(CliError::Gate(format!(
            "the provenance chain for {} is contradicted by its evidence: {}",
            target.contract,
            outcome.summary()
        )));
    }

    Ok(())
}

/// Parses a SHA-256 hex digest.
///
/// # Errors
///
/// Returns a usage error when the value is not a well-formed SHA-256 digest.
pub fn parse_digest(value: &str) -> CliResult<Digest> {
    Digest::new(DigestAlgorithm::Sha256, value)
        .map_err(|error| CliError::Usage(format!("{value} is not a SHA-256 hex digest: {error}")))
}

/// The published JSON view.
fn view(
    inspection: &amasario_contract::ContractInspection,
    outcome: &VerificationOutcome,
    observation: Option<&EvidenceRecord>,
    comparison: Option<&EvidenceRecord>,
    claimed: &ProvenanceArgs,
    chain: &ProvenanceChain,
) -> CliResult<Value> {
    let mut links: Vec<Value> = Vec::with_capacity(chain.links.len());
    for link in &chain.links {
        let subject = to_value(&link.subject)?;
        let object = link.object.as_ref().map(to_value).transpose()?;
        let confidence = to_value(&link.confidence)?;
        links.push(json!({
            "kind": link.kind.as_str(),
            "subject": subject,
            "object": object,
            "basis": link.basis.as_str(),
            "verification": link.verification.as_str(),
            "confidence": confidence,
        }));
    }

    Ok(json!({
        "contract": inspection.identity.contract_id.to_string(),
        "network": inspection.identity.network_id,
        "recordedDigest": inspection.identity.wasm_hash.as_ref().map(Digest::value),
        "moduleDigestVerified": inspection.digest_verified,
        "status": outcome.status.as_str(),
        "summary": outcome.summary(),
        "links": links,
        "missingStages": outcome
            .missing_links
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<&str>>(),
        "contradictedStages": outcome
            .contradictions
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<&str>>(),
        "evidence": {
            "observation": observation.map(|record| record.id.clone()),
            "comparison": comparison.map(|record| record.id.clone()),
        },
        "claimedSource": {
            "repository": claimed.source_repo,
            "revision": claimed.revision,
            "note": "A claimed source is recorded here and nowhere else. The engine holds \
                     no evidence for it, so it is not a link in the chain and does not \
                     raise the status.",
        },
        "boundary": to_value(&inspection.boundary)?,
    }))
}

/// The human-readable view.
fn text(
    outcome: &VerificationOutcome,
    claimed: &ProvenanceArgs,
    chain: &ProvenanceChain,
    inspection: &amasario_contract::ContractInspection,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "provenance  {}", inspection.identity.contract_id);
    let _ = writeln!(
        out,
        "digest      {}",
        inspection
            .identity
            .wasm_hash
            .as_ref()
            .map_or("(no module)", Digest::value)
    );
    let _ = writeln!(out, "status      {}", outcome.status.as_str());
    let _ = writeln!(out, "summary     {}", outcome.summary());
    let _ = writeln!(out);

    if chain.links.is_empty() {
        let _ = writeln!(
            out,
            "no stage of the chain could be established from this observation."
        );
    } else {
        let _ = writeln!(out, "established stages:");
        for link in &chain.links {
            let _ = writeln!(
                out,
                "  {} [{}] {}",
                link.kind.as_str(),
                link.basis.as_str(),
                link.verification.as_str()
            );
        }
    }

    if !outcome.missing_links.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "stages with no evidence behind them: {}",
            outcome
                .missing_links
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<&str>>()
                .join(", ")
        );
    }

    if claimed.source_repo.is_some() || claimed.revision.is_some() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "a source was claimed ({} at {}) and is recorded as unverified: the engine \
             holds no evidence for it",
            claimed
                .source_repo
                .as_deref()
                .unwrap_or("(repository not stated)"),
            claimed
                .revision
                .as_deref()
                .unwrap_or("(revision not stated)")
        );
    }

    let _ = writeln!(out);
    let _ = write!(
        out,
        "Amasario is not a security scanner. A verified provenance chain states that the \
         evidence is consistent with the claim; it says nothing about the trustworthiness \
         of the contract."
    );
    out
}
