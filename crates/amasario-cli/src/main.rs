//! The `amasario` command-line interface.
//!
//! # What this binary is
//!
//! A thin, scriptable driver over the engine's reusable libraries. It parses arguments,
//! drives the analysis pipeline, renders the result and chooses an exit code. It holds no
//! analysis logic of its own: everything it reports is produced by a library crate, so the
//! CLI and an embedder who calls those crates directly get the same answers.
//!
//! # Exit codes
//!
//! A run exits zero when it completed and the result passed any gate it was asked to
//! enforce, and non-zero otherwise. The non-zero codes are classified: a usage problem
//! exits 2, a failed gate exits 1, and an engine failure exits with a code derived from the
//! specification's error category, so a CI job can tell a network failure from a
//! provenance contradiction without parsing the message. See
//! [`errors::category_exit_code`].
//!
//! # Where the output goes
//!
//! The result goes to stdout or to `--output`; narration goes to stderr. A pipeline may
//! therefore consume stdout without stripping commentary.

mod commands;
mod config;
mod errors;
mod output;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::commands::diff::DiffArgs;
use crate::commands::export::ExportArgs;
use crate::commands::graph::GraphArgs;
use crate::commands::impact::ImpactArgs;
use crate::commands::provenance::ProvenanceArgs;
use crate::commands::report::ReportArgs;
use crate::commands::snapshot::SnapshotArgs;
use crate::commands::verify::VerifyArgs;
use crate::config::{BoundsArgs, OutputArgs, TargetArgs};
use crate::errors::{CliError, CliResult};

/// The Amasario contract dependency, provenance and impact engine.
#[derive(Debug, Parser)]
#[command(
    name = "amasario",
    version,
    about = "Soroban contract dependency, provenance and impact analysis",
    long_about = "Amasario is Soroban contract dependency, provenance and impact infrastructure. It \
                  answers what a contract depends on, where its deployed artifact came from, what \
                  evidence supports those relationships, and what could be affected by a change.\n\n\
                  Amasario is not a security scanner. Nothing it reports is a statement that a \
                  contract is secure, safe, malicious or vulnerable.",
    propagate_version = true,
    max_term_width = 100
)]
struct Cli {
    /// The analysis to perform.
    #[command(subcommand)]
    command: Command,
}

/// The analysis commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect a contract: identity, executable, interface and observed activity.
    Inspect {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        output: OutputArgs,
    },

    /// List the contracts observed in the target's company, without deciding dependencies.
    Discover {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        output: OutputArgs,
    },

    /// Establish where the deployed executable came from, and how well.
    Provenance {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        provenance: ProvenanceArgs,
        #[command(flatten)]
        output: OutputArgs,
    },

    /// Resolve what the contract depends on, with a basis and evidence per edge.
    Dependencies {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        output: OutputArgs,
    },

    /// Build the dependency graph and export it.
    Graph {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        graph: GraphArgs,
    },

    /// Analyse what a change to an entity could reach.
    Impact {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        impact: ImpactArgs,
        #[command(flatten)]
        output: OutputArgs,
    },

    /// Capture a normalised snapshot, or read one back.
    Snapshot {
        #[command(flatten)]
        args: SnapshotArgs,
    },

    /// Compare two snapshots.
    Diff {
        #[command(flatten)]
        args: DiffArgs,
    },

    /// Check whether the evidence supports the executable's identity, as a gate.
    Verify {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        verify: VerifyArgs,
        #[command(flatten)]
        output: OutputArgs,
    },

    /// Generate the specification's report in any of its four renderings.
    Report {
        #[command(flatten)]
        args: ReportArgs,
    },

    /// Export a recorded analysis as JSON, YAML, GraphML or DOT.
    Export {
        #[command(flatten)]
        args: ExportArgs,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing();

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            output::note(&format!("amasario: {error}"));
            ExitCode::from(error.exit_code())
        },
    }
}

/// Dispatches the parsed command.
///
/// The runtime is built here rather than annotated onto `main`, so that the commands which
/// do no I/O - `diff` and `export` - do not pay for a thread pool they never use, and so
/// that a failure to build one is reported as a usage problem rather than a panic.
fn run(cli: Cli) -> CliResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Usage(format!("could not start the async runtime: {error}")))?;

    runtime.block_on(async move {
        match cli.command {
            Command::Inspect {
                target,
                bounds,
                output,
            } => commands::inspect::run(target, bounds, output).await,
            Command::Discover {
                target,
                bounds,
                output,
            } => commands::discover::run(target, bounds, output).await,
            Command::Provenance {
                target,
                bounds,
                provenance,
                output,
            } => commands::provenance::run(target, bounds, provenance, output).await,
            Command::Dependencies {
                target,
                bounds,
                output,
            } => commands::dependencies::run(target, bounds, output).await,
            Command::Graph {
                target,
                bounds,
                graph,
            } => commands::graph::run(target, bounds, graph).await,
            Command::Impact {
                target,
                bounds,
                impact,
                output,
            } => commands::impact::run(target, bounds, impact, output).await,
            Command::Snapshot { args } => commands::snapshot::run(args).await,
            Command::Diff { args } => commands::diff::run(args),
            Command::Verify {
                target,
                bounds,
                verify,
                output,
            } => commands::verify::run(target, bounds, verify, output).await,
            Command::Report { args } => commands::report::run(args).await,
            Command::Export { args } => commands::export::run(args),
        }
    })
}

/// Initialises structured logging, to stderr so that stdout stays machine-readable.
///
/// The default level is `warn`, because a CLI that narrated at `info` on every run would
/// bury its own result. `RUST_LOG` raises it when an operator wants the detail.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    // A second initialisation fails, which happens when a library has already installed a
    // subscriber. That is not worth failing a run over.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_parses_every_command() {
        // Parsing is checked in one place so that a command whose arguments cannot be
        // constructed fails a test rather than a user's first run.
        for args in [
            vec!["amasario", "inspect", "--contract", "CABC"],
            vec!["amasario", "discover", "--contract", "CABC"],
            vec!["amasario", "provenance", "--contract", "CABC"],
            vec![
                "amasario",
                "dependencies",
                "--contract",
                "CABC",
                "--depth",
                "3",
            ],
            vec!["amasario", "graph", "--contract", "CABC", "--format", "dot"],
            vec!["amasario", "impact", "--contract", "CABC"],
            vec!["amasario", "snapshot", "create", "--contract", "CABC"],
            vec![
                "amasario", "diff", "--before", "a.json", "--after", "b.json",
            ],
            vec!["amasario", "verify", "--contract", "CABC"],
            vec![
                "amasario",
                "report",
                "--contract",
                "CABC",
                "--format",
                "markdown",
            ],
            vec![
                "amasario", "export", "--input", "g.json", "--format", "graphml",
            ],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_ok(),
                "failed to parse: {args:?}"
            );
        }
    }

    #[test]
    fn the_binary_is_the_product_name() {
        assert!(Cli::command().get_name().contains("amasario"));
    }

    /// The repository root, from this crate's manifest rather than the working directory,
    /// because a test's working directory is the package root.
    fn repository_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the crate sits two levels under the repository root")
            .to_path_buf()
    }

    /// A YAML document read from the repository, parsed.
    fn yaml_relative(path: &str) -> serde_norway::Value {
        let full = repository_root().join(path);
        let text = std::fs::read_to_string(&full)
            .unwrap_or_else(|error| panic!("{error}: {}", full.display()));
        serde_norway::from_str(&text)
            .unwrap_or_else(|error| panic!("{path} is not valid YAML: {error}"))
    }

    #[test]
    fn every_workflow_parses_as_yaml() {
        // A workflow that does not parse does not run, and the failure mode is a
        // repository that looks green because its gate silently stopped executing. This
        // is the cheapest possible check on that, and it runs in the crate that the
        // workflows drive.
        let workflows = repository_root().join(".github/workflows");
        let entries = std::fs::read_dir(&workflows).expect("the workflows directory exists");

        let mut seen = 0_usize;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "yml") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a file name")
                .to_owned();
            let document = yaml_relative(&format!(".github/workflows/{name}"));
            assert!(
                document.get("jobs").is_some_and(|jobs| !jobs.is_null()),
                ".github/workflows/{name} declares no jobs, so it can never do anything"
            );
            seen += 1;
        }

        assert!(seen >= 9, "only {seen} workflows were found and parsed");
    }

    #[test]
    fn the_composite_action_names_only_commands_the_cli_has() {
        // The action is how another project adopts the engine, and its `case` statement is
        // the allow-list of subcommands it will run. A subcommand renamed here and not
        // there would be a broken action discovered by a stranger rather than by CI, so the
        // list is checked against the command surface this crate actually declares.
        let action = yaml_relative(".github/actions/amasario/action.yml");

        assert_eq!(
            action["runs"]["using"].as_str(),
            Some("composite"),
            "the action must be composite: the engine needs no container"
        );
        assert_eq!(
            action["inputs"]["contract"]["required"].as_bool(),
            Some(true),
            "an analysis with no contract is not an analysis"
        );

        // The `case` alternatives, read out of the step that runs the engine rather than
        // duplicated here, because a copy in the test is a copy that stops matching.
        let steps = action["runs"]["steps"]
            .as_sequence()
            .expect("the action has steps");
        let script = steps
            .iter()
            .filter_map(|step| step["run"].as_str())
            .find(|script| script.contains("AMASARIO_COMMAND"))
            .expect("a step that dispatches on the command");

        let pattern = script
            .lines()
            .map(str::trim)
            .find(|line| line.contains('|') && line.ends_with(") ;;"))
            .expect("the command allow-list");
        let alternatives = pattern
            .strip_suffix(") ;;")
            .expect("the allow-list line ends the case pattern")
            .trim_end()
            .split('|')
            .collect::<Vec<_>>();

        assert!(
            alternatives.len() >= 9,
            "the action allows fewer commands than the CLI has: {alternatives:?}"
        );

        let declared = Cli::command()
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_owned())
            .collect::<Vec<_>>();

        for alternative in &alternatives {
            assert!(
                declared.iter().any(|name| name == alternative),
                "the action runs `{alternative}`, which this CLI does not declare; \
                 it declares {declared:?}"
            );
        }

        // And the reverse, for the commands that need no extra arguments: a subcommand
        // added here and never offered there is a gap a reader would not notice.
        for name in &declared {
            if matches!(name.as_str(), "help" | "diff" | "export") {
                // `diff` and `export` read files rather than a network, and `help` is
                // clap's own; none of the three is a contract analysis, which is the only
                // thing the action exists to run.
                continue;
            }
            assert!(
                alternatives.contains(&name.as_str()),
                "`{name}` is a contract analysis but the action does not offer it"
            );
        }
    }
}
