//! `amasario graph` - the target's dependency graph, in a form a consumer can read.
//!
//! The graph is built from the resolved dependency set, so every edge in it carries an
//! identifier, a relationship, evidence citations and a confidence, and the document
//! records whether the edge was observed or inferred. A picture that drew an inference in
//! the same style as an observation would be the renderer's most damaging possible
//! mistake, so the DOT rendering distinguishes them and the JSON document carries the
//! flag on the edge itself.
//!
//! # The document is validated before it is written
//!
//! [`Graph::validate`](amasario_graph::Graph::validate) runs first, so an export cannot
//! publish a graph with a dangling edge or an unannotated node. A graph that failed its
//! own integrity check would be worse than no export, because a consumer would treat it
//! as authoritative.

use std::path::PathBuf;

use amasario_export::export_graph;
use clap::Args;

use crate::config::{BoundsArgs, GraphFormat, TargetArgs};
use crate::errors::CliResult;
use crate::output::emit;

use super::{dependencies_of, graph_of, observe, subject_of};

/// The graph-specific arguments.
#[derive(Debug, Clone, Args)]
pub struct GraphArgs {
    /// The export format.
    #[arg(long, value_enum, default_value_t = GraphFormat::Dot)]
    pub format: GraphFormat,

    /// Write to this path instead of stdout.
    #[arg(long, short, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Indent JSON and YAML output for a person to read.
    #[arg(long)]
    pub pretty: bool,
}

/// Runs the command.
///
/// # Errors
///
/// Returns the observation's failure, a dependency failure while resolving edges, a graph
/// failure when the assembled graph fails its integrity check, and an export failure when
/// the requested format cannot represent the document.
pub async fn run(target: TargetArgs, bounds: BoundsArgs, graph_args: GraphArgs) -> CliResult<()> {
    let inspection = observe(&target, &bounds).await?;
    let subject = subject_of(&target)?;
    let set = dependencies_of(&subject, &inspection, bounds.depth)?;
    let graph = graph_of(&subject, &set)?;

    // The graph's own integrity check, run before anything is written.
    graph.validate()?;

    let document = amasario_graph::GraphDocument::of(&graph);
    let format = graph_args.format.export_format();
    let mut rendered = export_graph(&document, format)?;

    // JSON and YAML are the two formats a person is likely to open in an editor, and
    // `--pretty` applies to them and to nothing else: GraphML and DOT have a canonical
    // layout of their own and reflowing them would change what a tool sees.
    if graph_args.pretty && matches!(format, amasario_export::Format::Json) {
        let value: serde_json::Value = serde_json::from_str(&rendered)?;
        rendered = serde_json::to_string_pretty(&value)?;
    }

    emit(&rendered, graph_args.output.as_deref())?;
    Ok(())
}
