//! `amasario dependencies` - what the target depends on, and on what evidence.
//!
//! The command resolves the target's dependency set from observed cross-contract
//! invocations, classifies each edge, and reports the set. Every edge carries a basis, at
//! least one evidence citation and a verification status, because the specification
//! forbids a dependency without them: a relationship that no observation supports is not
//! a low-confidence dependency, it is not a dependency at all.
//!
//! # What the command refuses to imply
//!
//! It does not merge the target's callers into its dependencies. A contract that calls
//! the target is not something the target depends on, and the direction matters for
//! impact. Invocations made by another contract are set aside by the detection pass with
//! a reason, and that reason reaches the report through the set's own accounting rather
//! than being silently dropped.

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::{dependencies_of, observe, subject_of, to_value};

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a dependency failure when a candidate could not be
/// resolved, and a usage error for a format the command cannot render.
pub async fn run(target: TargetArgs, bounds: BoundsArgs, output: OutputArgs) -> CliResult<()> {
    let inspection = observe(&target, &bounds).await?;
    let subject = subject_of(&target)?;
    let set = dependencies_of(&subject, &inspection, bounds.depth)?;

    let rendered = match output.format {
        OutputFormat::Json => {
            let document =
                amasario_dependency::DependencySetDocument::of(&set)?.with_id(format!("{subject}"));
            let value = to_value(&document)?;
            if output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&target.contract, &set, &inspection),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "dependencies renders text or json, not {}; use `graph` for DOT",
                output.format.as_str()
            )));
        },
    };

    emit(&rendered, output.output.as_deref())?;
    Ok(())
}

/// The human-readable listing.
fn text(
    contract: &str,
    set: &amasario_dependency::DependencySet,
    inspection: &amasario_contract::ContractInspection,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "dependencies for {contract}");
    let _ = writeln!(
        out,
        "depth bound  {}; {} direct, {} transitive, {} unestablished",
        set.max_depth,
        set.direct.len(),
        set.transitive.len(),
        set.unestablished.len()
    );
    let _ = writeln!(
        out,
        "evidence     {} invocation(s) observed from {} transaction(s) read",
        inspection.invocations.len(),
        inspection.transactions_read
    );
    let _ = writeln!(out);

    if set.is_empty() {
        let _ = writeln!(
            out,
            "no dependency was established. This is the absence of evidence, not evidence \
             of the absence of a dependency: a call that left no trace on the chain cannot \
             be observed."
        );
    } else {
        for dependency in set.all() {
            let classes: Vec<&str> = dependency
                .classes
                .iter()
                .map(|class| class.as_str())
                .collect();
            let _ = writeln!(
                out,
                "{} -> {} [{}]",
                dependency.subject,
                dependency.object,
                dependency.relationship.as_str()
            );
            let _ = writeln!(out, "  class       {}", classes.join(", "));
            let _ = writeln!(out, "  basis       {}", dependency.basis.as_str());
            let _ = writeln!(
                out,
                "  verification {}{}",
                dependency.verification.as_str(),
                if dependency.depth == 0 {
                    String::new()
                } else {
                    format!(", {} hop(s) away", dependency.depth)
                }
            );
            let _ = writeln!(
                out,
                "  confidence  {} ({} evidence citation(s))",
                dependency.confidence.level.as_str(),
                dependency.evidence.len()
            );
            let _ = writeln!(out, "  because     {}", dependency.reason);
            if !dependency.path.is_empty() {
                let path: Vec<String> = dependency.path.iter().map(ToString::to_string).collect();
                let _ = writeln!(
                    out,
                    "  path        {} -> {} -> {}",
                    dependency.subject,
                    path.join(" -> "),
                    dependency.object
                );
            }
            let _ = writeln!(out);
        }
    }

    if !set.unestablished.is_empty() {
        let _ = writeln!(
            out,
            "{} candidate(s) could not be established as dependencies:",
            set.unestablished.len()
        );
        for unestablished in &set.unestablished {
            let _ = writeln!(
                out,
                "  {} -> {} [{}]: {}",
                set.subject,
                unestablished.object,
                unestablished.relationship.as_str(),
                unestablished.reason
            );
        }
        let _ = writeln!(out);
    }

    if !set.cycles.is_empty() {
        let _ = writeln!(
            out,
            "{} cycle(s) were found and reported rather than resolved:",
            set.cycles.len()
        );
        for cycle in &set.cycles {
            let members: Vec<&str> = cycle
                .entities
                .iter()
                .map(|entity| entity.id.as_str())
                .collect();
            let _ = writeln!(out, "  {}", members.join(" -> "));
        }
        let _ = writeln!(out);
    }

    if set.truncated {
        let _ = writeln!(
            out,
            "this dependency set is bounded and is not a claim of completeness"
        );
        let _ = writeln!(out);
    }

    let _ = write!(
        out,
        "Amasario is dependency infrastructure, not a security scanner. A dependency \
         recorded here is not a statement about the dependency's trustworthiness."
    );
    out
}
