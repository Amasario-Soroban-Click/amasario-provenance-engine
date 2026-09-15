//! `amasario export` - convert a recorded analysis into another format.
//!
//! # Reading a document the engine wrote
//!
//! The command reads a snapshot or a graph document and writes it in one of the four
//! formats, so that an analysis captured once can be rendered for a graph tool, embedded in
//! a document, or archived without loss. It does not re-run the analysis: the input
//! carries the observation boundary, and re-deriving it at a later boundary would produce a
//! different result under the same name.
//!
//! # Lossless and lossy
//!
//! JSON and YAML round-trip; GraphML and DOT do not, and the export crate records which is
//! which rather than claiming all four are equivalent. A caller archiving an analysis uses
//! a lossless one, and this command does not pretend otherwise.

use std::path::PathBuf;

use amasario_export::export_graph;
use amasario_graph::GraphDocument;
use clap::Args;

use crate::config::GraphFormat;
use crate::errors::{CliError, CliResult};
use crate::output::{emit, note};

/// The export arguments.
#[derive(Debug, Clone, Args)]
pub struct ExportArgs {
    /// The document to read: a snapshot, or a graph document, as JSON.
    #[arg(long, value_name = "FILE")]
    pub input: PathBuf,

    /// The format to write.
    #[arg(long, value_enum, default_value_t = GraphFormat::Graphml)]
    pub format: GraphFormat,

    /// Write to this path instead of stdout.
    #[arg(long, short, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Indent JSON output for a person to read.
    #[arg(long)]
    pub pretty: bool,
}

/// Runs the command.
///
/// # Errors
///
/// Returns a snapshot failure when the input looks like a snapshot but cannot be read, a
/// graph failure when a graph document cannot be parsed, and an export failure when the
/// requested format cannot represent the graph.
pub fn run(args: ExportArgs) -> CliResult<()> {
    let text = std::fs::read_to_string(&args.input).map_err(|error| {
        CliError::Usage(format!(
            "{} could not be read: {error}",
            args.input.display()
        ))
    })?;

    let document = load_graph(&text, &args.input)?;
    let format = args.format.export_format();
    let mut rendered = export_graph(&document, format)?;

    if args.pretty && matches!(format, amasario_export::Format::Json) {
        let value: serde_json::Value = serde_json::from_str(&rendered)?;
        rendered = serde_json::to_string_pretty(&value)?;
    }

    note(&format!(
        "exported {} node(s) and {} edge(s) as {}",
        document.nodes.len(),
        document.edges.len(),
        format.as_str()
    ));
    emit(&rendered, args.output.as_deref())?;
    Ok(())
}

/// Loads a graph document from a snapshot or a standalone graph.
///
/// A snapshot is tried first because its envelope identifies it unambiguously, and a
/// failure to read one is not reported as an error: a file that is not a snapshot is
/// expected to be a graph.
///
/// # Errors
///
/// Returns a validation error when the input is neither a snapshot nor a graph document,
/// and a graph error when a snapshot carries no graph.
fn load_graph(text: &str, path: &std::path::Path) -> CliResult<GraphDocument> {
    if let Ok(snapshot) = amasario_snapshot::from_json(text, &path.display().to_string()) {
        return snapshot.graph.ok_or_else(|| {
            CliError::Usage(format!(
                "{} is a snapshot but carries no graph, so there is nothing to export; \
                 capture it with a dependency analysis first",
                path.display()
            ))
        });
    }

    serde_json::from_str::<GraphDocument>(text).map_err(|error| {
        CliError::Usage(format!(
            "{} is neither a snapshot nor a graph document: {error}",
            path.display()
        ))
    })
}
