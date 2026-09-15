//! `amasario snapshot` - capture a normalised analysis, or read one back.
//!
//! # Why a snapshot is not just a saved report
//!
//! A snapshot is the engine's own record of an analysis at one boundary, normalised so
//! that two runs over the same facts produce the same bytes and compared field by field
//! rather than as text. That is what makes `diff` able to say "these two edges are the
//! same edge" instead of reporting every edge as replaced. The content digest covers
//! everything except the fields the specification declares volatile, so a snapshot's
//! digest is stable across runs and a change to it means a change in what was observed.
//!
//! # It will not capture nothing
//!
//! [`Capture::build`](amasario_snapshot::Capture::build) refuses a snapshot with no
//! evidence, and this command does not help it: the observation of the executable digest is
//! always recorded, so a snapshot always has something behind it. A snapshot that had no
//! evidence would satisfy the schema only by inventing a record.

use std::path::{Path, PathBuf};

use amasario_core::ENGINE_VERSION;
use amasario_evidence::artifact::from_executable;
use amasario_provenance::{ArtifactIdentity, ArtifactType};
use amasario_snapshot::store;
use clap::{Args, Subcommand};

use crate::config::{BoundsArgs, OutputArgs, OutputFormat, TargetArgs, now_rfc3339};
use crate::errors::{CliError, CliResult};
use crate::output::{emit, note};

use super::{dependencies_of, graph_of, observe, subject_of, to_value};

/// The snapshot subcommands.
#[derive(Debug, Clone, Args)]
pub struct SnapshotArgs {
    /// Which action to perform.
    #[command(subcommand)]
    pub action: SnapshotAction,
}

/// The snapshot actions.
#[derive(Debug, Clone, Subcommand)]
pub enum SnapshotAction {
    /// Capture the current state of a contract.
    Create(CreateArgs),
    /// Read a snapshot and report what it holds.
    Show(ShowArgs),
}

/// The `create` arguments.
#[derive(Debug, Clone, Args)]
pub struct CreateArgs {
    /// The target contract and network.
    #[command(flatten)]
    pub target: TargetArgs,

    /// The bounds the analysis runs under.
    #[command(flatten)]
    pub bounds: BoundsArgs,

    /// The directory to write the snapshot into. Defaults to the current directory.
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub output: PathBuf,
}

/// The `show` arguments.
#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    /// The snapshot file to read.
    #[arg(long, value_name = "FILE")]
    pub input: PathBuf,

    /// How to render the summary.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a snapshot failure when the document cannot be built
/// or written, and a usage error for a format the action cannot render.
pub async fn run(args: SnapshotArgs) -> CliResult<()> {
    match args.action {
        SnapshotAction::Create(create) => create_snapshot(create).await,
        SnapshotAction::Show(show) => show_snapshot(show),
    }
}

/// Captures and writes a snapshot.
async fn create_snapshot(args: CreateArgs) -> CliResult<()> {
    let inspection = observe(&args.target, &args.bounds).await?;
    let subject = subject_of(&args.target)?;
    let set = dependencies_of(&subject, &inspection, args.bounds.depth)?;
    let graph = graph_of(&subject, &set)?;
    graph.validate()?;
    let document = amasario_graph::GraphDocument::of(&graph);

    let contract = inspection.identity.contract_id.clone();
    let mut capture =
        amasario_snapshot::Capture::of(inspection.identity.clone(), inspection.boundary.clone())
            .with_dependencies(set.clone())
            .with_graph(document)
            .with_engine_version(ENGINE_VERSION)
            .truncated(inspection.is_truncated() || set.truncated);

    // The observation the snapshot rests on. A snapshot with no evidence is refused, and
    // this is the record that keeps it from being empty.
    if let Some(digest) = inspection.identity.wasm_hash.clone() {
        let record = from_executable(
            digest.clone(),
            &contract,
            inspection.boundary.clone(),
            format!("evidence:wasm:{contract}"),
            inspection.boundary.observed_at,
        )?;
        let evidence_id = record.id.clone();
        capture.add_evidence(record)?;
        capture = capture.with_wasm(ArtifactIdentity::new(
            digest,
            ArtifactType::Wasm,
            vec![evidence_id],
        )?);
    }

    let snapshot = capture.build(now_rfc3339())?;
    snapshot.validate()?;

    let path = store::write(&snapshot, &args.output)?;
    note(&format!("wrote {}", path.display()));
    note(&format!("snapshot {}", snapshot.id));
    note(&format!(
        "content digest {}",
        snapshot.content_digest.value()
    ));

    Ok(())
}

/// Reads a snapshot and reports what it holds.
fn show_snapshot(args: ShowArgs) -> CliResult<()> {
    let snapshot = store::read(&args.input)?;

    let rendered = match args.output.format {
        OutputFormat::Json => {
            let value = to_value(&snapshot)?;
            if args.output.pretty {
                serde_json::to_string_pretty(&value)?
            } else {
                serde_json::to_string(&value)?
            }
        },
        OutputFormat::Text => show_text(&snapshot, &args.input),
        OutputFormat::Markdown | OutputFormat::Dot | OutputFormat::Junit => {
            return Err(CliError::Usage(format!(
                "snapshot show renders text or json, not {}",
                args.output.format.as_str()
            )));
        },
    };

    emit(&rendered, args.output.output.as_deref())?;
    Ok(())
}

/// The human-readable summary of a snapshot.
fn show_text(snapshot: &amasario_snapshot::Snapshot, path: &Path) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "snapshot    {}", snapshot.id);
    let _ = writeln!(out, "read from   {}", path.display());
    let _ = writeln!(out, "captured    {}", snapshot.captured_at);
    let _ = writeln!(out, "contract    {}", snapshot.contract.contract_id);
    let _ = writeln!(out, "network     {}", snapshot.network.id);
    let _ = writeln!(out, "at ledger   {}", snapshot.ledger_boundary);
    let _ = writeln!(
        out,
        "engine      {}",
        snapshot
            .engine_version
            .as_deref()
            .unwrap_or("(not recorded)")
    );
    let _ = writeln!(out, "evidence    {} record(s)", snapshot.evidence.len());
    let _ = writeln!(out, "confidence  {}", snapshot.confidence.level.as_str());
    let _ = writeln!(
        out,
        "graph       {} node(s), {} edge(s)",
        snapshot.graph.as_ref().map_or(0, |graph| graph.nodes.len()),
        snapshot.graph.as_ref().map_or(0, |graph| graph.edges.len())
    );
    let _ = writeln!(out, "digest      {}", snapshot.content_digest.value());
    if snapshot.truncated {
        let _ = writeln!(
            out,
            "bounded     the captured analysis stopped early, so the snapshot does not claim \
             completeness"
        );
    }
    let _ = writeln!(out);
    let _ = write!(
        out,
        "A snapshot records what was observed at one boundary. It is not a security \
         assessment."
    );
    out
}
