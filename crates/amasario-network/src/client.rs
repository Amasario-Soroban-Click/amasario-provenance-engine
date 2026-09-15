//! Endpoints, network identities and the resolution of a requested network into
//! something the engine can contact.
//!
//! # Where every fact in this module comes from
//!
//! The passphrases, the network ids, and which endpoints are SDF-operated are
//! **Stellar protocol facts**, not design choices, and they are taken from the
//! Stellar documentation rather than recalled:
//! <https://developers.stellar.org/docs/networks>. Two consequences are worth
//! stating because they shape the API.
//!
//! First, the network id is the SHA-256 of the passphrase, so the engine derives
//! it and the tests assert the derivation against the three published ids. A
//! hardcoded id that drifted from its passphrase would be a silent lie, and
//! deriving it makes that impossible.
//!
//! Second, and more importantly for how this module is shaped: **SDF operates
//! public RPC endpoints for Testnet and Futurenet only.** The documentation is
//! explicit that Mainnet RPC is available from third-party providers, and lists no
//! SDF-operated Mainnet endpoint
//! (<https://developers.stellar.org/docs/data/apis/rpc/providers>). The engine
//! therefore does *not* ship a Mainnet default. Reporting a fabricated default
//! would be worse than reporting nothing, because an analysis of Mainnet performed
//! against an endpoint the engine assumed exists is an analysis of a network
//! nobody chose. Mainnet requires an explicit endpoint, and
//! [`KnownNetwork::default_rpc`] returns `None` for it so that the requirement is
//! visible in the type rather than only in a message.
//!
//! # Credentials
//!
//! [`RpcEndpoint::new`] and [`HorizonEndpoint::new`] reject a URL carrying user
//! information. This is a privacy requirement rather than tidiness: the endpoint
//! string is recorded on every error, written into reports and snapshots, and a
//! URL such as `https://user:token@host` would place a credential in all of them.
//! Provider URLs that carry an API key in the *path*, as some do, cannot be
//! rejected here because the path is not distinguishable from a legitimate route;
//! the specification's privacy chapter and the CLI's secret handling are where
//! that case is addressed.

use std::fmt;

use amasario_core::{EngineConfig, EngineError, Network, NetworkType, Result, RetryPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

/// A validated Stellar RPC endpoint.
///
/// Newtype rather than a bare `String` so that an endpoint which reached the
/// network layer is known to have passed validation, and so that no caller can
/// build one that embeds a credential.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RpcEndpoint(String);

/// A validated Stellar Horizon endpoint.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HorizonEndpoint(String);

/// Validates a base URL, rejecting anything the engine cannot contact or must not
/// record.
fn validate_base_url(url: &str, field: &str) -> Result<String> {
    let parsed = Url::parse(url).map_err(|error| {
        EngineError::Configuration(format!("{field} {url:?} is not a valid URL: {error}"))
    })?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(EngineError::Configuration(format!(
            "{field} {url:?} uses scheme {:?}; only http and https are supported",
            parsed.scheme()
        )));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(EngineError::Configuration(format!(
            "{field} {url:?} has no host"
        )));
    }
    // Rejected because the endpoint string travels into errors, reports and
    // snapshots, and a credential must not travel with it.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EngineError::Configuration(format!(
            "{field} must not embed credentials; supply them through the environment instead"
        )));
    }

    Ok(url.to_owned())
}

impl RpcEndpoint {
    /// Validates an RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] for a URL that cannot be parsed, is
    /// not `http`/`https`, has no host, or embeds user information.
    pub fn new(url: impl Into<String>) -> Result<Self> {
        let url = url.into();
        validate_base_url(&url, "RPC endpoint").map(Self)
    }

    /// The endpoint as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl HorizonEndpoint {
    /// Validates a Horizon endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] for a URL that cannot be parsed, is
    /// not `http`/`https`, has no host, or embeds user information.
    pub fn new(url: impl Into<String>) -> Result<Self> {
        let url = url.into();
        validate_base_url(&url, "Horizon endpoint").map(Self)
    }

    /// The endpoint as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RpcEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for HorizonEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RpcEndpoint {
    type Error = EngineError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<RpcEndpoint> for String {
    fn from(value: RpcEndpoint) -> Self {
        value.0
    }
}

impl TryFrom<String> for HorizonEndpoint {
    type Error = EngineError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<HorizonEndpoint> for String {
    fn from(value: HorizonEndpoint) -> Self {
        value.0
    }
}

/// The Stellar networks whose identities the engine knows.
///
/// The set is closed. An unrecognised network is rejected rather than accepted as
/// a `CUSTOM` network, because the engine cannot verify an endorsement it does not
/// understand, and silently treating an unknown name as custom is how an operator
/// ends up analysing the wrong chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KnownNetwork {
    /// The dev network for features ahead of Testnet.
    Futurenet,
    /// The public test network, operated by SDF.
    Testnet,
    /// The public production network.
    Mainnet,
}

/// The SDF-operated Futurenet RPC endpoint.
pub const FUTURENET_RPC: &str = "https://rpc-futurenet.stellar.org";
/// The SDF-operated Testnet RPC endpoint.
pub const TESTNET_RPC: &str = "https://soroban-testnet.stellar.org";
/// The SDF-operated Futurenet Horizon endpoint.
pub const FUTURENET_HORIZON: &str = "https://horizon-futurenet.stellar.org";
/// The SDF-operated Testnet Horizon endpoint.
pub const TESTNET_HORIZON: &str = "https://horizon-testnet.stellar.org";

impl KnownNetwork {
    /// The network's passphrase, as published by Stellar.
    #[must_use]
    pub const fn passphrase(self) -> &'static str {
        match self {
            Self::Futurenet => "Test SDF Future Network ; October 2022",
            Self::Testnet => "Test SDF Network ; September 2015",
            Self::Mainnet => "Public Global Stellar Network ; September 2015",
        }
    }

    /// The network id as published by Stellar.
    ///
    /// Present so that [`KnownNetwork::network_id`] can be checked against an
    /// independent statement of the same fact. The tests do exactly that.
    #[must_use]
    pub const fn published_network_id(self) -> &'static str {
        match self {
            Self::Futurenet => "a3a1c6a78286713e29be0e9785670fa838d13917cd8eaeb4a3579ff1debc7fd5",
            Self::Testnet => "cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472",
            Self::Mainnet => "7ac33997544e3175d266bd022439b22cdb16508c01163f26e5cb2a3e1045a979",
        }
    }

    /// The engine's classification of this network.
    #[must_use]
    pub const fn network_type(self) -> NetworkType {
        match self {
            Self::Futurenet => NetworkType::Futurenet,
            Self::Testnet => NetworkType::Testnet,
            Self::Mainnet => NetworkType::Mainnet,
        }
    }

    /// The name the CLI accepts.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Futurenet => "futurenet",
            Self::Testnet => "testnet",
            Self::Mainnet => "mainnet",
        }
    }

    /// The network id, derived as the SHA-256 of the passphrase.
    ///
    /// Derived rather than hardcoded: the passphrase is the fact, and an id that
    /// disagreed with its passphrase would identify a network the engine is not
    /// actually talking to.
    #[must_use]
    pub fn network_id(self) -> String {
        network_id_for_passphrase(self.passphrase())
    }

    /// An SDF-operated default endpoint, where one exists.
    ///
    /// `None` for Mainnet because SDF does not operate a public Mainnet RPC
    /// endpoint. Returning `None` rather than a plausible-looking URL is the point:
    /// see the module documentation.
    #[must_use]
    pub const fn default_rpc(self) -> Option<&'static str> {
        match self {
            Self::Futurenet => Some(FUTURENET_RPC),
            Self::Testnet => Some(TESTNET_RPC),
            Self::Mainnet => None,
        }
    }

    /// An SDF-operated default Horizon endpoint, where one exists.
    #[must_use]
    pub const fn default_horizon(self) -> Option<&'static str> {
        match self {
            Self::Futurenet => Some(FUTURENET_HORIZON),
            Self::Testnet => Some(TESTNET_HORIZON),
            Self::Mainnet => None,
        }
    }

    /// Every known network.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Futurenet, Self::Testnet, Self::Mainnet]
    }
}

impl fmt::Display for KnownNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for KnownNetwork {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "futurenet" => Ok(Self::Futurenet),
            "testnet" => Ok(Self::Testnet),
            "mainnet" | "pubnet" | "public" => Ok(Self::Mainnet),
            other => Err(EngineError::Configuration(format!(
                "unrecognised network {other:?}; expected one of futurenet, testnet, mainnet"
            ))),
        }
    }
}

/// The network id for a passphrase: its SHA-256, lowercase hex.
///
/// Exposed because the engine can be pointed at a network whose passphrase it
/// knows but whose identity it has no published constant for - a private
/// deployment, for instance - and the derivation is the protocol rule.
#[must_use]
pub fn network_id_for_passphrase(passphrase: &str) -> String {
    let digest = Sha256::digest(passphrase.as_bytes());
    hex::encode(digest)
}

/// Everything needed to contact a network: its identity and its endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkTarget {
    network: Network,
    rpc: RpcEndpoint,
    horizon: Option<HorizonEndpoint>,
    retry: RetryPolicy,
}

impl NetworkTarget {
    /// Resolves a known network into a target, requiring an explicit RPC endpoint
    /// where none is operated.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Configuration`] when the network has no default RPC
    /// endpoint and none was supplied. The message names the reason and points at
    /// the documentation, so that the failure is actionable rather than mysterious.
    pub fn resolve(
        known: KnownNetwork,
        rpc_override: Option<&str>,
        horizon_override: Option<&str>,
        config: &EngineConfig,
    ) -> Result<Self> {
        let rpc = match (rpc_override, known.default_rpc()) {
            (Some(url), _) => RpcEndpoint::new(url)?,
            (None, Some(url)) => RpcEndpoint::new(url)?,
            (None, None) => {
                return Err(EngineError::Configuration(format!(
                    "no default RPC endpoint exists for {known}: SDF operates public RPC endpoints \
                     for Futurenet and Testnet only, and Mainnet RPC is served by third parties. \
                     Supply an endpoint explicitly. See \
                     https://developers.stellar.org/docs/data/apis/rpc/providers"
                )));
            },
        };

        let horizon = match (horizon_override, known.default_horizon()) {
            (Some(url), _) => Some(HorizonEndpoint::new(url)?),
            (None, Some(url)) => Some(HorizonEndpoint::new(url)?),
            (None, None) => None,
        };

        let network = Network::new(known.label(), known.network_type(), known.passphrase())?
            .with_rpc_url(rpc.as_str());

        Ok(Self {
            network,
            rpc,
            horizon,
            retry: config.retry,
        })
    }

    /// Builds a target for a network whose endpoints are all supplied explicitly.
    ///
    /// Used for a private or provider-hosted deployment, where the passphrase is
    /// known but no published constant describes the operator's infrastructure.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Validation`] for an empty passphrase or name, and
    /// [`EngineError::Configuration`] for an endpoint that fails validation.
    pub fn custom(
        name: &str,
        passphrase: &str,
        rpc: &str,
        horizon: Option<&str>,
        config: &EngineConfig,
    ) -> Result<Self> {
        let rpc = RpcEndpoint::new(rpc)?;
        let horizon = horizon.map(HorizonEndpoint::new).transpose()?;
        // Classified as CUSTOM rather than as whichever known network shares the
        // passphrase: the endpoints are the operator's, and the classification
        // drives whether a mutating operation would require an opt-in.
        let network =
            Network::new(name, NetworkType::Custom, passphrase)?.with_rpc_url(rpc.as_str());

        Ok(Self {
            network,
            rpc,
            horizon,
            retry: config.retry,
        })
    }

    /// The network descriptor, which carries the passphrase that identifies the
    /// chain and the endpoint the observations came from.
    #[must_use]
    pub const fn network(&self) -> &Network {
        &self.network
    }

    /// The RPC endpoint.
    #[must_use]
    pub const fn rpc(&self) -> &RpcEndpoint {
        &self.rpc
    }

    /// The Horizon endpoint, when one is configured.
    #[must_use]
    pub const fn horizon(&self) -> Option<&HorizonEndpoint> {
        self.horizon.as_ref()
    }

    /// The retry policy in force.
    #[must_use]
    pub const fn retry(&self) -> &RetryPolicy {
        &self.retry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EngineConfig {
        EngineConfig::default()
    }

    #[test]
    fn every_published_network_id_is_the_sha256_of_its_passphrase() {
        // This is the assertion that makes the module's facts self-checking. If a
        // passphrase were transcribed wrongly, the derived id would stop matching
        // the published one and this test would fail, rather than the engine
        // silently analysing a chain it is not identifying correctly.
        for &known in KnownNetwork::all() {
            assert_eq!(
                known.network_id(),
                known.published_network_id(),
                "{known} network id must be the SHA-256 of its passphrase"
            );
        }
    }

    #[test]
    fn the_network_id_derivation_matches_the_documented_testnet_value() {
        // Spelled out independently of the table above, so that a transcription
        // error in `published_network_id` cannot make the previous test pass by
        // agreeing with itself.
        assert_eq!(
            network_id_for_passphrase("Test SDF Network ; September 2015"),
            "cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472"
        );
    }

    #[test]
    fn mainnet_has_no_default_rpc_endpoint() {
        // SDF operates public RPC endpoints for Futurenet and Testnet only, so a
        // Mainnet default would be an invention.
        assert!(KnownNetwork::Mainnet.default_rpc().is_none());
        assert!(KnownNetwork::Mainnet.default_horizon().is_none());
        assert!(KnownNetwork::Testnet.default_rpc().is_some());
        assert!(KnownNetwork::Futurenet.default_rpc().is_some());
    }

    #[test]
    fn resolving_mainnet_without_an_endpoint_fails_with_an_actionable_reason() {
        let error = NetworkTarget::resolve(KnownNetwork::Mainnet, None, None, &config())
            .expect_err("mainnet has no default endpoint");
        let message = error.to_string();
        assert!(message.contains("Mainnet"), "got: {message}");
        assert!(
            message.contains("public RPC endpoints for Futurenet and Testnet only"),
            "the reason must be stated: {message}"
        );
        assert!(
            message.contains("developers.stellar.org"),
            "the message must point somewhere actionable: {message}"
        );
    }

    #[test]
    fn resolving_mainnet_with_an_explicit_endpoint_succeeds() {
        let target = NetworkTarget::resolve(
            KnownNetwork::Mainnet,
            Some("https://stellar.api.onfinality.io/public"),
            None,
            &config(),
        )
        .expect("an explicit endpoint is enough");
        assert_eq!(
            target.network().passphrase,
            "Public Global Stellar Network ; September 2015"
        );
        assert!(target.horizon().is_none());
    }

    #[test]
    fn resolving_testnet_uses_the_sdf_endpoints() {
        let target = NetworkTarget::resolve(KnownNetwork::Testnet, None, None, &config())
            .expect("testnet has defaults");
        assert_eq!(target.rpc().as_str(), TESTNET_RPC);
        assert_eq!(
            target.horizon().map(HorizonEndpoint::as_str),
            Some(TESTNET_HORIZON)
        );
        assert_eq!(target.network().network_type, NetworkType::Testnet);
    }

    #[test]
    fn an_explicit_endpoint_overrides_the_default_without_losing_the_network_identity() {
        // The passphrase identifies the chain and the endpoint is where it is read
        // from; overriding one must not change the other.
        let target = NetworkTarget::resolve(
            KnownNetwork::Testnet,
            Some("http://127.0.0.1:8000"),
            None,
            &config(),
        )
        .expect("an override is permitted");
        assert_eq!(target.rpc().as_str(), "http://127.0.0.1:8000");
        assert_eq!(
            target.network().passphrase,
            "Test SDF Network ; September 2015"
        );
    }

    #[test]
    fn an_endpoint_embedding_credentials_is_rejected() {
        // The endpoint string is written into errors, reports and snapshots, so a
        // credential in it would be published in all three.
        let error = RpcEndpoint::new("https://user:secret@rpc.example.test")
            .expect_err("credentials must be rejected");
        assert!(error.to_string().contains("must not embed credentials"));
        assert!(HorizonEndpoint::new("https://token@horizon.example.test").is_err());
    }

    #[test]
    fn an_endpoint_that_is_not_http_is_rejected() {
        for url in [
            "ftp://rpc.example.test",
            "file:///etc/passwd",
            "rpc.example.test",
            "https://",
        ] {
            assert!(
                RpcEndpoint::new(url).is_err(),
                "{url} must not be accepted as an endpoint"
            );
        }
    }

    #[test]
    fn a_valid_endpoint_round_trips_through_its_serialised_form() {
        let endpoint = RpcEndpoint::new(TESTNET_RPC).expect("valid");
        let json = serde_json::to_string(&endpoint).expect("serialises");
        assert_eq!(json, format!("\"{TESTNET_RPC}\""));
        let decoded: RpcEndpoint = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(decoded, endpoint);
    }

    #[test]
    fn a_deserialised_endpoint_is_validated_rather_than_trusted() {
        // A snapshot or configuration file is input like any other. Accepting an
        // unvalidated endpoint from a document would let a credential-bearing URL
        // re-enter the engine through a file.
        serde_json::from_str::<RpcEndpoint>("\"https://user:secret@rpc.example.test\"")
            .expect_err("deserialisation must validate");
    }

    #[test]
    fn a_custom_network_is_classified_as_custom_even_when_it_shares_a_passphrase() {
        let target = NetworkTarget::custom(
            "private-testnet",
            "Test SDF Network ; September 2015",
            "https://rpc.internal.example.test",
            None,
            &config(),
        )
        .expect("a private deployment is representable");
        assert_eq!(target.network().network_type, NetworkType::Custom);
        // Which matters because the classification, not the passphrase, decides
        // whether a mutating operation needs an explicit opt-in.
        assert!(
            target
                .network()
                .network_type
                .requires_explicit_opt_in_for_mutation()
        );
    }

    #[test]
    fn a_custom_network_with_an_empty_name_or_passphrase_is_rejected() {
        assert!(NetworkTarget::custom("", "passphrase", TESTNET_RPC, None, &config()).is_err());
        assert!(NetworkTarget::custom("name", "", TESTNET_RPC, None, &config()).is_err());
    }

    #[test]
    fn the_network_names_the_cli_accepts_round_trip() {
        for &known in KnownNetwork::all() {
            assert_eq!(
                known.label().parse::<KnownNetwork>().expect("round trip"),
                known
            );
        }
        // The aliases Stellar's own tooling uses for the public network.
        assert_eq!(
            "pubnet".parse::<KnownNetwork>().expect("alias"),
            KnownNetwork::Mainnet
        );
        assert_eq!(
            "MAINNET".parse::<KnownNetwork>().expect("case insensitive"),
            KnownNetwork::Mainnet
        );
        assert!("sandbox".parse::<KnownNetwork>().is_err());
    }

    #[test]
    fn every_known_network_declares_a_network_type_matching_its_name() {
        assert_eq!(KnownNetwork::Mainnet.network_type(), NetworkType::Mainnet);
        assert_eq!(KnownNetwork::Testnet.network_type(), NetworkType::Testnet);
        assert_eq!(
            KnownNetwork::Futurenet.network_type(),
            NetworkType::Futurenet
        );
        // Only Mainnet observations are authoritative, which the endpoint
        // validation relies on when it refuses to treat an unclassified endpoint
        // as safe.
        assert!(KnownNetwork::Mainnet.network_type().is_authoritative());
        assert!(!KnownNetwork::Testnet.network_type().is_authoritative());
    }
}
