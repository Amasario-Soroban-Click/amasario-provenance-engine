//! The argument groups every command shares, and the resolution they perform.
//!
//! # Grouped rather than repeated
//!
//! Ten of the eleven commands take a target contract on a named network, and all of them
//! write their result somewhere in some format. Declaring those as reusable groups means
//! the help text, the default, and the validation of `--network` are written once, so a
//! command cannot quietly accept a network name another command rejects.
//!
//! # Resolution happens before anything is contacted
//!
//! [`TargetArgs::resolve`] turns a network name and any endpoint overrides into a
//! [`NetworkTarget`] and refuses the combinations that cannot work - a mainnet target with
//! no RPC endpoint, an endpoint that is not a URL - before a connection is attempted. A
//! run that cannot succeed therefore fails with the argument that caused it rather than
//! with a transport error several seconds later.

use std::path::PathBuf;

use amasario_contract::DEFAULT_MAX_TRANSACTION_READS;
use amasario_core::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_NODES, DepthBounds, EngineConfig, EngineError, Result,
};
use amasario_network::{
    DEFAULT_LOOKBACK_LEDGERS, DEFAULT_MAX_EVENT_PAGES, KnownNetwork, NetworkTarget,
};
use clap::{Args, ValueEnum};

/// The network a command observes, and how to reach it.
#[derive(Debug, Clone, Args)]
pub struct TargetArgs {
    /// The contract address to analyse.
    #[arg(long, value_name = "CONTRACT")]
    pub contract: String,

    /// The network to observe: futurenet, testnet or mainnet.
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Override the RPC endpoint. Required for a network with no published endpoint.
    #[arg(long, value_name = "URL")]
    pub rpc: Option<String>,

    /// Override the Horizon endpoint, which enables deployment resolution.
    #[arg(long, value_name = "URL")]
    pub horizon: Option<String>,

    /// The network's passphrase, for a network the engine has no published identity for.
    ///
    /// A local standalone network, a private deployment and a partner environment all
    /// identify themselves by passphrase, and none of them is one of the three networks
    /// the engine ships constants for. Supplying the passphrase is what lets the engine
    /// check that the endpoint really serves that chain, which is the check that would
    /// otherwise have to be skipped.
    #[arg(long, value_name = "PASSPHRASE", requires = "network_name")]
    pub passphrase: Option<String>,

    /// The name to record for a custom network. Defaults to `custom`.
    #[arg(long = "network-name", value_name = "NAME", requires = "passphrase")]
    pub network_name: Option<String>,
}

impl TargetArgs {
    /// Resolves the network name and any endpoint overrides into a target.
    ///
    /// # Errors
    ///
    /// Returns a configuration error naming the accepted networks when the name is not
    /// one of them, and a configuration error when the network has no published RPC
    /// endpoint and none was supplied. Mainnet is the case that makes the second matter:
    /// SDF publishes no default endpoint for it, so pretending otherwise would produce a
    /// mysterious transport error instead of a clear one.
    ///
    /// When `--passphrase` is supplied the network is resolved as a custom one instead:
    /// the passphrase identifies the chain and the endpoints are whatever was supplied.
    /// `--rpc` is then required, because a custom network has no published default.
    pub fn resolve(&self) -> Result<NetworkTarget> {
        let config = EngineConfig::default();

        if let Some(passphrase) = &self.passphrase {
            let name = self.network_name.as_deref().unwrap_or("custom");
            let rpc = self.rpc.as_deref().ok_or_else(|| {
                EngineError::Configuration(
                    "--passphrase names a network the engine has no published endpoint for, so \
                     --rpc is required"
                        .to_owned(),
                )
            })?;
            return NetworkTarget::custom(name, passphrase, rpc, self.horizon.as_deref(), &config);
        }

        let known: KnownNetwork = self.network.parse()?;
        NetworkTarget::resolve(known, self.rpc.as_deref(), self.horizon.as_deref(), &config)
    }
}

/// Where a result goes and how it is written.
#[derive(Debug, Clone, Args)]
pub struct OutputArgs {
    /// The format to render.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Write to this path instead of stdout.
    #[arg(long, short, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Indent JSON and YAML output for a person to read.
    #[arg(long)]
    pub pretty: bool,
}

/// The renderings a command can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// A human-readable summary.
    Text,
    /// The specification's document, as JSON.
    Json,
    /// A report, as Markdown.
    Markdown,
    /// A graph, as Graphviz DOT.
    Dot,
    /// A report, as JUnit XML.
    Junit,
}

impl OutputFormat {
    /// The stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Dot => "dot",
            Self::Junit => "junit",
        }
    }
}

/// The export formats a graph can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GraphFormat {
    /// The specification's graph document, as JSON.
    Json,
    /// The same document, as YAML.
    Yaml,
    /// GraphML, for graph tooling.
    Graphml,
    /// Graphviz DOT.
    Dot,
}

impl GraphFormat {
    /// The export format this maps to.
    #[must_use]
    pub const fn export_format(self) -> amasario_export::Format {
        match self {
            Self::Json => amasario_export::Format::Json,
            Self::Yaml => amasario_export::Format::Yaml,
            Self::Graphml => amasario_export::Format::GraphML,
            Self::Dot => amasario_export::Format::Dot,
        }
    }
}

/// The bounds a traversal runs under.
#[derive(Debug, Clone, Args)]
pub struct BoundsArgs {
    /// How many edges traversal may cross before it must stop and say so.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MAX_DEPTH)]
    pub depth: u32,

    /// The most entities a traversal may accumulate.
    #[arg(long = "max-nodes", value_name = "N", default_value_t = DEFAULT_MAX_NODES)]
    pub max_nodes: usize,

    /// The most transactions to read for invocation evidence.
    #[arg(
        long = "max-transactions",
        value_name = "N",
        default_value_t = DEFAULT_MAX_TRANSACTION_READS
    )]
    pub max_transactions: usize,

    /// Scan the contract's events, which is what finds cross-contract calls.
    #[arg(long)]
    pub scan_events: bool,

    /// How far back an event scan reaches, in ledgers, ending at the boundary.
    ///
    /// The default is one day. This is the choice that decides whether a dependency
    /// answer describes the present or a week ago: an event page carries as many
    /// events as its limit allows, so a scan that starts at the oldest ledger a node
    /// retains spends its whole page budget on the oldest few minutes of the
    /// retention window and never reaches recent activity.
    #[arg(
        long,
        value_name = "LEDGERS",
        default_value_t = DEFAULT_LOOKBACK_LEDGERS,
        conflicts_with = "from_ledger"
    )]
    pub lookback: u32,

    /// Scan forward from this ledger instead of over a recent window.
    ///
    /// For a deliberate historical analysis. The range is still clamped to what the
    /// endpoint retains, and the result says when it was.
    #[arg(long, value_name = "LEDGER", conflicts_with = "lookback")]
    pub from_ledger: Option<u32>,

    /// How many event pages the scan may read before it stops and says so.
    #[arg(
        long = "max-event-pages",
        value_name = "N",
        default_value_t = DEFAULT_MAX_EVENT_PAGES
    )]
    pub max_event_pages: usize,
}

impl Default for BoundsArgs {
    fn default() -> Self {
        Self {
            depth: DEFAULT_MAX_DEPTH,
            max_nodes: DEFAULT_MAX_NODES,
            max_transactions: DEFAULT_MAX_TRANSACTION_READS,
            scan_events: false,
            lookback: DEFAULT_LOOKBACK_LEDGERS,
            from_ledger: None,
            max_event_pages: DEFAULT_MAX_EVENT_PAGES,
        }
    }
}

impl BoundsArgs {
    /// The engine configuration these bounds describe.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when a bound is outside the range the specification
    /// permits, so that an unbounded traversal is refused before it begins rather than
    /// caught part-way through.
    pub fn engine_config(&self) -> Result<EngineConfig> {
        let config = EngineConfig {
            bounds: DepthBounds {
                max_depth: self.depth,
                max_nodes: self.max_nodes,
            },
            ..EngineConfig::default()
        };
        config.validate()?;
        Ok(config)
    }
}

/// The current time, as an RFC 3339 timestamp.
///
/// Read here rather than inside the engine, because two runs over the same input must
/// produce the same output and a clock read inside an analysis would break that. The CLI
/// is where a run's wall-clock time enters, and it is the field every digest excludes.
#[must_use]
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        bounds: BoundsArgs,
        #[command(flatten)]
        output: OutputArgs,
    }

    fn parse(args: &[&str]) -> Harness {
        Harness::try_parse_from(std::iter::once("amasario").chain(args.iter().copied()))
            .expect("the arguments parse")
    }

    #[test]
    fn a_contract_and_a_default_network_are_enough() {
        let harness = parse(&["--contract", "CABC"]);
        assert_eq!(harness.target.network, "testnet");
        assert_eq!(harness.bounds.depth, DEFAULT_MAX_DEPTH);
        assert_eq!(harness.bounds.max_nodes, DEFAULT_MAX_NODES);
        assert_eq!(harness.output.format, OutputFormat::Text);
        assert!(harness.output.output.is_none());
    }

    #[test]
    fn testnet_resolves_to_its_published_endpoint() {
        let harness = parse(&["--contract", "CABC", "--network", "testnet"]);
        let target = harness.target.resolve().expect("testnet resolves");
        assert!(target.rpc().as_str().contains("testnet"));
        assert!(target.horizon().is_some());
    }

    #[test]
    fn mainnet_without_an_endpoint_is_refused_rather_than_guessed_at() {
        // SDF publishes no mainnet RPC endpoint, so a run that named one and supplied no
        // endpoint cannot work. Failing here names the cause.
        let harness = parse(&["--contract", "CABC", "--network", "mainnet"]);
        let error = harness.target.resolve().expect_err("no default endpoint");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::Configuration
        );
    }

    #[test]
    fn an_endpoint_override_is_honoured() {
        let harness = parse(&[
            "--contract",
            "CABC",
            "--network",
            "mainnet",
            "--rpc",
            "https://rpc.example.test",
        ]);
        let target = harness.target.resolve().expect("the override resolves");
        assert_eq!(target.rpc().as_str(), "https://rpc.example.test");
    }

    #[test]
    fn an_unknown_network_names_the_ones_that_exist() {
        let harness = parse(&["--contract", "CABC", "--network", "localnet"]);
        let error = harness.target.resolve().expect_err("not a known network");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::Configuration
        );
    }

    #[test]
    fn a_depth_beyond_the_permitted_maximum_is_refused() {
        let harness = parse(&[
            "--contract",
            "CABC",
            "--depth",
            &(amasario_core::MAX_PERMITTED_DEPTH + 1).to_string(),
        ]);
        let error = harness
            .bounds
            .engine_config()
            .expect_err("the depth is out of range");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::Configuration
        );
    }

    #[test]
    fn a_valid_depth_builds_a_configuration() {
        let harness = parse(&["--contract", "CABC", "--depth", "3"]);
        let config = harness
            .bounds
            .engine_config()
            .expect("a valid configuration");
        assert_eq!(config.bounds.max_depth, 3);
    }

    #[test]
    fn a_custom_network_is_identified_by_its_passphrase() {
        // A local standalone network identifies itself by passphrase, and it is not one of
        // the three the engine ships constants for. Supplying the passphrase is what lets
        // the network identity check run instead of being skipped.
        let harness = parse(&[
            "--contract",
            "CABC",
            "--passphrase",
            "Standalone Network ; February 2017",
            "--network-name",
            "local",
            "--rpc",
            "http://localhost:8000/soroban/rpc",
        ]);
        let target = harness.target.resolve().expect("a custom network resolves");
        assert_eq!(
            target.network().passphrase,
            "Standalone Network ; February 2017"
        );
        assert_eq!(target.network().id, "local");
        assert_eq!(target.rpc().as_str(), "http://localhost:8000/soroban/rpc");
    }

    #[test]
    fn a_custom_network_without_an_endpoint_is_refused() {
        // A custom network has no published default endpoint, so a run without one would
        // have nowhere to observe and must fail with the argument that caused it.
        let harness = parse(&[
            "--contract",
            "CABC",
            "--passphrase",
            "Standalone Network ; February 2017",
            "--network-name",
            "local",
        ]);
        let error = harness
            .target
            .resolve()
            .expect_err("an endpoint is required");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::Configuration
        );
        assert!(error.to_string().contains("--rpc"), "got: {error}");
    }

    #[test]
    fn a_passphrase_without_a_network_name_is_a_usage_error() {
        // The two flags describe one thing, so half of it is not an option.
        let parsed =
            Harness::try_parse_from(["amasario", "--contract", "CABC", "--passphrase", "x"]);
        assert!(parsed.is_err(), "--passphrase alone must be rejected");
    }

    #[test]
    fn a_timestamp_is_rfc_3339() {
        let timestamp = now_rfc3339();
        // Parsing it back is the only check that matters: a consumer will.
        let parsed =
            time::OffsetDateTime::parse(&timestamp, &time::format_description::well_known::Rfc3339);
        assert!(parsed.is_ok(), "not RFC 3339: {timestamp}");
    }
}
