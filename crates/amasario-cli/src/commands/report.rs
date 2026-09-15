//! `amasario report` - the analysis, in the four renderings the specification defines.
//!
//! # The sections stay apart
//!
//! A report is not a summary. The specification's central requirement is that observed
//! facts, inferred relationships, verification outcomes, confidence, unknown information
//! and errors occupy distinct sections, because an inference printed in the same voice as
//! an observation is the failure that makes provenance tooling untrustworthy. Every
//! renderer this command drives keeps that separation, and the Markdown rendering repeats
//! each inference's basis rather than trusting the section heading to have been read.
//!
//! # What is unknown is said
//!
//! The engine cannot see a contract's source revision or build from the chain alone. That
//! is recorded in the `unknown` section rather than omitted, so a reader is told what the
//! analysis could not determine instead of being left to infer that there was nothing to
//! determine. The anonymous absence is what the section exists to prevent.

use amasario_core::{Confidence, ConfidenceLevel, EntityKind, EntityRef};
use amasario_graph::GraphDocument;
use amasario_report::model::{
    ConfidenceEntry, InferredStatement, RelationshipFinding, Report, Statement, UnknownEntry,
    UnknownReason, VerificationEntry,
};
use amasario_report::{Format as ReportFormat, render, render_summary};
use clap::Args;

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs, now_rfc3339};
use crate::errors::CliResult;
use crate::output::emit;

use super::{dependencies_of, graph_of, observe, subject_of};

/// The report arguments.
#[derive(Debug, Clone, Args)]
pub struct ReportArgs {
    /// The target contract and network.
    #[command(flatten)]
    pub target: TargetArgs,

    /// The bounds the analysis runs under.
    #[command(flatten)]
    pub bounds: BoundsArgs,

    /// How to render the report.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a report failure when a section cannot be assembled
/// or the document fails its structural checks, and a usage error for an unknown format.
pub async fn run(args: ReportArgs) -> CliResult<()> {
    let inspection = observe(&args.target, &args.bounds).await?;
    let subject = subject_of(&args.target)?;
    let set = dependencies_of(&subject, &inspection, args.bounds.depth)?;
    let graph = graph_of(&subject, &set)?;
    graph.validate()?;
    let document = GraphDocument::of(&graph);

    let contract = inspection.identity.contract_id.clone();
    let observation_id = format!("evidence:wasm:{contract}");

    let mut report = Report::new(subject.clone())
        .with_boundary(inspection.boundary.clone())
        .generated_at(now_rfc3339())
        .truncated(inspection.is_truncated() || set.truncated)
        .with_graph(document.clone());

    // The observation the rest of the report rests on.
    if let Some(digest) = &inspection.identity.wasm_hash {
        report.add_observed(Statement::new(
            format!(
                "at ledger {} on {} the contract at {contract} reports the executable hash {}",
                inspection.boundary.ledger.get(),
                inspection.boundary.network.id,
                digest.value()
            ),
            Some(subject.clone()),
            vec![observation_id.clone()],
        )?);
    }

    // The verification outcome for the executable's identity.
    let (status, detail) = match (&inspection.identity.wasm_hash, inspection.digest_verified) {
        (None, _) => (
            amasario_core::VerificationStatus::Unknown,
            "the contract executes no module, so there is no executable identity to verify"
                .to_owned(),
        ),
        (Some(_), Some(true)) => (
            amasario_core::VerificationStatus::Verified,
            "the retrieved module's bytes hash to the digest the network records".to_owned(),
        ),
        (Some(_), Some(false)) => (
            amasario_core::VerificationStatus::Conflicting,
            "the retrieved module's bytes do not hash to the digest the network records".to_owned(),
        ),
        (Some(_), None) => (
            amasario_core::VerificationStatus::Unverified,
            "the digest was recorded but the module's bytes were not retrieved".to_owned(),
        ),
    };
    report.add_verification(
        VerificationEntry::new(subject.clone(), status)
            .with_detail(detail)
            .with_evidence(vec![observation_id.clone()]),
    );

    // Confidence for the target, with the evidence it derives from.
    let level = match status {
        amasario_core::VerificationStatus::Verified => ConfidenceLevel::HighConfidence,
        amasario_core::VerificationStatus::PartiallyVerified => ConfidenceLevel::MediumConfidence,
        amasario_core::VerificationStatus::Unverified => ConfidenceLevel::LowConfidence,
        _ => ConfidenceLevel::Unknown,
    };
    let evidence = if inspection.identity.wasm_hash.is_some() {
        vec![observation_id]
    } else {
        // A confidence must cite something. The boundary is the observation that the
        // contract exists at all, which is what remains when there is no module.
        vec![format!(
            "observation:contract:{contract}@{}",
            inspection.boundary.ledger.get()
        )]
    };
    report.add_confidence(ConfidenceEntry {
        subject,
        confidence: Confidence::new(level, evidence, Vec::new())?,
    });

    // Every edge: an explanation of why it exists, and a statement in the section that
    // matches whether it was observed or derived.
    for edge in &document.edges {
        report.add_relationship_finding(RelationshipFinding::of_edge(edge)?);

        let statement = format!(
            "{} is held between {} and {}",
            edge.relationship.as_str(),
            edge.source,
            edge.target
        );
        let entity = EntityRef::new(EntityKind::Contract, edge.target.as_str())?;
        if edge.observed {
            report.add_observed(Statement::new(
                statement,
                Some(entity),
                edge.evidence.clone(),
            )?);
        } else {
            report.add_inferred(InferredStatement::new(
                statement,
                Some(entity),
                edge.evidence.clone(),
                "the relationship was inferred from the evidence rather than observed \
                 directly, and is reported in this section so that it is not read in the \
                 same voice as an observation",
            )?);
        }
    }

    // What the engine could not determine, stated rather than omitted.
    report.add_unknown(
        UnknownEntry::new(
            "What source revision was the deployed executable built from?",
            UnknownReason::EvidenceUnavailable,
        )?
        .with_detail(
            "no source record was supplied, and the chain alone does not name one. This is \
             not a statement that the contract has no source.",
        ),
    );
    report.add_unknown(
        UnknownEntry::new(
            "Which toolchain and configuration produced the deployed artifact?",
            UnknownReason::EvidenceUnavailable,
        )?
        .with_detail("a build record would be required, and none is available at this boundary."),
    );

    let rendered = match args.output.format {
        OutputFormat::Text => render_summary(&report)?,
        OutputFormat::Json => {
            let value: serde_json::Value =
                serde_json::from_str(&render(&report, ReportFormat::Json)?)?;
            if args.output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Markdown => render(&report, ReportFormat::Markdown)?,
        OutputFormat::Dot => render(&report, ReportFormat::Dot)?,
        OutputFormat::Junit => render(&report, ReportFormat::Junit)?,
    };

    emit(&rendered, args.output.output.as_deref())?;
    Ok(())
}
