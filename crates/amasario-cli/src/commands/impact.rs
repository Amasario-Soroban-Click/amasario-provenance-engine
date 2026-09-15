//! `amasario impact` - what a change to an entity could reach.
//!
//! The command builds the target's dependency graph, then propagates a change from one
//! entity through the relationships that carry it. Given `A -> B -> C`, a change to `B`
//! reaches `A` and `C`, and each finding records the path, its hop depth, the evidence and
//! the confidence.
//!
//! # A bounded result is not an exhausted one
//!
//! When propagation stops at the depth bound the command says so on stdout as well as in
//! the document, because "nothing further is affected" and "the search stopped before it
//! knew" produce the same finding list and mean opposite things. The distinction is the
//! one the specification's `impact-propagation` rule is written to preserve.
//!
//! # The change type is not guessed
//!
//! `--change-type` is optional, and when it is absent the findings carry no change
//! classification rather than an assumed `MODIFIED`. The specification's taxonomy
//! restricts `UPGRADED` and `DOWNGRADED` to an established ordering and forbids Amasario
//! from guessing at one, so the engine has nothing to guess with either.

use amasario_core::{EntityKind, EntityRef};
use amasario_dependency::Limits;
use amasario_impact::{ChangeType, ImpactAnalysis};
use clap::Args;
use serde_json::{Value, json};

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::{dependencies_of, graph_of, observe, subject_of, to_value};

/// The impact-specific arguments.
#[derive(Debug, Clone, Args)]
pub struct ImpactArgs {
    /// The entity that changed. Defaults to the target contract itself.
    #[arg(long, value_name = "CONTRACT")]
    pub changed: Option<String>,

    /// The kind of change: one of ADDED, REMOVED, MODIFIED, UPGRADED, DOWNGRADED,
    /// REPLACED. Omitted means the findings carry no change classification.
    #[arg(long = "change-type", value_name = "TYPE")]
    pub change_type: Option<String>,
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a dependency failure while resolving edges, an
/// impact failure when a bound is zero, and a usage error for an unknown change type or an
/// unrenderable format.
pub async fn run(
    target: TargetArgs,
    bounds: BoundsArgs,
    impact: ImpactArgs,
    output: OutputArgs,
) -> CliResult<()> {
    let inspection = observe(&target, &bounds).await?;
    let subject = subject_of(&target)?;
    let set = dependencies_of(&subject, &inspection, bounds.depth)?;
    let graph = graph_of(&subject, &set)?;

    let changed = match &impact.changed {
        Some(address) => EntityRef::new(EntityKind::Contract, address.as_str())?,
        None => subject,
    };
    let change_type = impact
        .change_type
        .as_deref()
        .map(parse_change_type)
        .transpose()?;

    let limits = Limits::new(bounds.depth as usize, bounds.max_nodes)?;
    let context = amasario_impact::context(&graph, &[]);
    let analysis = amasario_impact::analyze(&context, &changed, change_type, limits)?;

    let rendered = match output.format {
        OutputFormat::Json => {
            let value = view(&analysis, change_type)?;
            if output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&analysis, change_type),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "impact renders text or json, not {}; use `report` for those",
                output.format.as_str()
            )));
        },
    };

    emit(&rendered, output.output.as_deref())?;
    Ok(())
}

/// Parses a change type from its wire name.
///
/// Uses the type's own deserialiser rather than a table here, so a term added to the
/// taxonomy is accepted by the CLI in the same change that adds it to the engine.
///
/// # Errors
///
/// Returns a usage error naming the accepted terms.
pub fn parse_change_type(name: &str) -> CliResult<ChangeType> {
    let upper = name.trim().to_ascii_uppercase();
    serde_json::from_value::<ChangeType>(Value::String(upper)).map_err(|_| {
        CliError::Usage(format!(
            "{name} is not a change type; accepted: ADDED, REMOVED, MODIFIED, UPGRADED, \
             DOWNGRADED, REPLACED"
        ))
    })
}

/// The published JSON view of an analysis.
fn view(analysis: &ImpactAnalysis, change_type: Option<ChangeType>) -> CliResult<Value> {
    let mut findings: Vec<Value> = Vec::with_capacity(analysis.findings.len());
    for finding in &analysis.findings {
        let impact_types: Vec<&str> = finding
            .impact_type
            .iter()
            .map(|impact| impact.as_str())
            .collect();
        let changed_entity = to_value(&finding.changed_entity)?;
        let affected_entity = to_value(&finding.affected_entity)?;
        let confidence = to_value(&finding.confidence)?;
        findings.push(json!({
            "id": finding.id,
            "changedEntity": changed_entity,
            "affectedEntity": affected_entity,
            "impactType": impact_types,
            "hopDepth": finding.hop_depth,
            "path": finding.path.as_ref().map(|path| path.render()),
            "relationships": finding
                .relationship_types
                .iter()
                .map(|relationship| relationship.as_str())
                .collect::<Vec<&str>>(),
            "reason": finding.reason,
            "confidence": confidence,
            "verificationState": finding.verification_state.map(|state| state.as_str()),
        }));
    }

    Ok(json!({
        "changed": to_value(&analysis.changed)?,
        "changeType": change_type.map(ChangeType::as_str),
        "findings": findings,
        "nodesVisited": analysis.nodes_visited,
        "truncated": analysis.truncated,
        "truncationReason": analysis.truncation_reason.map(|reason| reason.as_str()),
        "conclusive": analysis.is_conclusive(),
        "note": "A finding states that an entity could be affected, on the strength of the \
                 relationships between it and the change. It is not a prediction that the \
                 change will break it.",
    }))
}

/// The human-readable view.
fn text(analysis: &ImpactAnalysis, change_type: Option<ChangeType>) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "impact of a change to {}", analysis.changed);
    let _ = writeln!(
        out,
        "change type  {}",
        change_type.map_or("(not stated)", ChangeType::as_str)
    );
    let _ = writeln!(
        out,
        "affected     {} entit(ies) over {} visited node(s), deepest {} hop(s)",
        analysis.findings.len().saturating_sub(1),
        analysis.nodes_visited,
        analysis.deepest()
    );
    let _ = writeln!(out);

    if analysis.findings.is_empty() {
        let _ = writeln!(out, "no entity is affected by this change.");
    } else {
        for finding in &analysis.findings {
            let types: Vec<&str> = finding
                .impact_type
                .iter()
                .map(|impact| impact.as_str())
                .collect();
            let _ = writeln!(
                out,
                "{} [{}] {} hop(s)",
                finding.affected_entity,
                types.join(", "),
                finding.hop_depth
            );
            if let Some(path) = &finding.path {
                let _ = writeln!(out, "  path        {}", path.render());
            }
            let _ = writeln!(out, "  because     {}", finding.reason);
            let _ = writeln!(out, "  confidence  {}", finding.confidence.level.as_str());
            let _ = writeln!(out);
        }
    }

    if analysis.truncated {
        let _ = writeln!(
            out,
            "the traversal stopped before it exhausted the graph ({}), so this is not a \
             claim of completeness",
            analysis
                .truncation_reason
                .map_or("reason not recorded", |reason| reason.as_str())
        );
        let _ = writeln!(out);
    }

    let _ = write!(
        out,
        "Amasario is not a security scanner, and a finding here is not a prediction of \
         breakage. It states what the recorded relationships could reach."
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_type_is_parsed_case_insensitively() {
        assert_eq!(
            parse_change_type("modified").expect("lowercase is accepted"),
            ChangeType::Modified
        );
        assert_eq!(
            parse_change_type("UPGRADED").expect("uppercase is accepted"),
            ChangeType::Upgraded
        );
    }

    #[test]
    fn an_unknown_change_type_names_the_accepted_terms() {
        let error = parse_change_type("broken").expect_err("not a change type");
        let rendered = error.to_string();
        assert!(rendered.contains("MODIFIED"), "got: {rendered}");
        assert!(rendered.contains("REPLACED"), "got: {rendered}");
    }
}
