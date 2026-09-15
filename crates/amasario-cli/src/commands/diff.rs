//! `amasario diff` - what changed between two snapshots.
//!
//! # Comparing identities, not text
//!
//! The comparison is canonical: both snapshots are normalised first, so the same edge
//! observed at two boundaries compares as the same edge unless something about it actually
//! changed. A textual diff of two JSON documents would report every reordered array as a
//! change, and a reader would learn to ignore it.
//!
//! # An incomparable pair is a result, not a failure
//!
//! Two snapshots from different networks, or from specification families this engine cannot
//! relate, produce a diff with `comparable: false` and a reason. That is deliberately not
//! an error: a caller that received an error could not tell "these cannot be compared" from
//! "the comparison itself failed", and only one of those is worth retrying.

use std::path::PathBuf;

use amasario_snapshot::store;
use clap::Args;

use crate::config::{OutputArgs, OutputFormat, now_rfc3339};
use crate::errors::{CliError, CliResult};
use crate::output::emit;

use super::to_value;

/// The diff arguments.
#[derive(Debug, Clone, Args)]
pub struct DiffArgs {
    /// The earlier snapshot.
    #[arg(long, value_name = "FILE")]
    pub before: PathBuf,

    /// The later snapshot.
    #[arg(long, value_name = "FILE")]
    pub after: PathBuf,

    /// How to render the difference.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs the command.
///
/// # Errors
///
/// Returns a snapshot failure when a file cannot be read or is not a snapshot, and a usage
/// error for a format the command cannot render.
pub fn run(args: DiffArgs) -> CliResult<()> {
    let before = store::read(&args.before)?;
    let after = store::read(&args.after)?;
    let diff = amasario_snapshot::compare(&before, &after, now_rfc3339())?;
    diff.validate()?;

    let rendered = match args.output.format {
        OutputFormat::Json => {
            let value = to_value(&diff)?;
            if args.output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => text(&diff, &args),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "diff renders text or json, not {}",
                args.output.format.as_str()
            )));
        },
    };

    emit(&rendered, args.output.output.as_deref())?;
    Ok(())
}

/// The human-readable view.
fn text(diff: &amasario_snapshot::Diff, args: &DiffArgs) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "diff        {} -> {}",
        args.before.display(),
        args.after.display()
    );
    let _ = writeln!(
        out,
        "snapshots   {} ({}) -> {} ({})",
        diff.before.id,
        diff.before.content_digest.value(),
        diff.after.id,
        diff.after.content_digest.value()
    );

    if !diff.comparable {
        let _ = writeln!(
            out,
            "comparable  no: {}",
            diff.incomparable_reason
                .map_or("reason not recorded", |reason| reason.as_str())
        );
        let _ = writeln!(
            out,
            "            the two snapshots do not describe comparable observations, so no \
             difference is reported"
        );
        return out;
    }

    match &diff.summary {
        Some(summary) => {
            let _ = writeln!(out, "changes     {}", summary.total);
            for (category, count) in &summary.by_category {
                let _ = writeln!(out, "  {category}: {count}");
            }
        },
        None => {
            let _ = writeln!(out, "changes     {}", diff.changes.len());
        },
    }
    let _ = writeln!(out);

    if diff.changes.is_empty() {
        let _ = writeln!(out, "no difference was detected between the two snapshots.");
    } else {
        for change in &diff.changes {
            let _ = writeln!(
                out,
                "{} [{}] {}{}",
                change.change_type.as_str(),
                change.category.as_str(),
                change.entity,
                change
                    .path
                    .as_deref()
                    .map_or(String::new(), |path| format!(" at {path}"))
            );
            let _ = writeln!(out, "  because     {}", change.reason);
            if let Some(before) = &change.before_value {
                let _ = writeln!(out, "  before      {before}");
            }
            if let Some(after) = &change.after_value {
                let _ = writeln!(out, "  after       {after}");
            }
        }
        let _ = writeln!(out);
        if diff.triggers_impact() {
            let _ = writeln!(
                out,
                "this difference changes the dependency or provenance surface, so an impact \
                 analysis from the after-state is warranted"
            );
        }
    }

    let _ = writeln!(out);
    let _ = write!(
        out,
        "A difference here is a change in what was observed, not an assessment of whether \
         the change is safe."
    );
    out
}
