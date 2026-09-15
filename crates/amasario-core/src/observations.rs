//! Observations and the boundaries that qualify them.
//!
//! Every fact the engine reports was observed at a point, and the specification
//! treats that point as part of the claim rather than as metadata: the same query
//! at a later ledger is a different analysis and may legitimately produce a
//! different answer. A fact without a boundary is not reproducible, which is why
//! [`ObservationBoundary`] is required on every produced document even when every
//! other field is optional.
//!
//! Two distinctions in this module do real work.
//!
//! An observation is not a creation fact. The ledger inside
//! [`ObservationBoundary`] records where something was *seen*, which can be later
//! than where it happened; a contract observed at ledger 900 may have been deployed
//! at ledger 700, and reporting the observation ledger as the deployment ledger
//! would upgrade an observation into a claim the evidence does not support.
//!
//! A truncation is not an absence. [`TruncationReason`] exists so that a bounded
//! traversal can say it was bounded, because "no more dependencies exist" and "the
//! search stopped" are different results that a consumer cannot tell apart after
//! the fact.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::errors::{EngineError, Result};
use crate::identity::LedgerSequence;

/// The network environments the specification classifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum NetworkType {
    /// A developer-controlled environment whose state is disposable.
    Local,
    /// A Stellar-operated environment for features ahead of testnet.
    Futurenet,
    /// The public Stellar test network.
    Testnet,
    /// The public Stellar production network.
    Mainnet,
    /// An operator-defined environment reachable through supplied endpoints.
    Custom,
}

impl NetworkType {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Futurenet => "FUTURENET",
            Self::Testnet => "TESTNET",
            Self::Mainnet => "MAINNET",
            Self::Custom => "CUSTOM",
        }
    }

    /// Every network type, in the order the specification's taxonomy lists them.
    ///
    /// Present so that the conformance test can compare this enumeration against
    /// `taxonomies/network-types.yaml` mechanically rather than by inspection.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Local,
            Self::Futurenet,
            Self::Testnet,
            Self::Mainnet,
            Self::Custom,
        ]
    }

    /// How durable an observation made here is.
    ///
    /// Recorded because a comparison across a boundary that may have been reset is
    /// not a comparison of one fact observed twice, and the engine must be able to
    /// refuse it.
    #[must_use]
    pub const fn durability(self) -> &'static str {
        match self {
            Self::Local => "ephemeral",
            Self::Futurenet => "reset_without_notice",
            Self::Testnet => "persistent_but_untrusted",
            Self::Mainnet => "authoritative",
            Self::Custom => "operator_defined",
        }
    }

    /// Whether observations here have production significance.
    #[must_use]
    pub const fn is_authoritative(self) -> bool {
        matches!(self, Self::Mainnet)
    }

    /// Whether a mutating operation must require an explicit opt-in.
    ///
    /// True for anything that is not a local environment. The taxonomy states the
    /// underlying prohibition directly - code that treats "not MAINNET" as "safe to
    /// mutate" is prohibited - because the reasoning is backwards: an unrecognised
    /// environment is unclassified, and the safe assumption about an unclassified
    /// environment is that it might be production.
    #[must_use]
    pub const fn requires_explicit_opt_in_for_mutation(self) -> bool {
        !matches!(self, Self::Local)
    }

    /// Whether two observations on this type of network can be compared without
    /// the environment possibly having changed underneath them.
    #[must_use]
    pub const fn is_durable_enough_to_compare(self) -> bool {
        matches!(self, Self::Mainnet | Self::Testnet)
    }
}

impl fmt::Display for NetworkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for NetworkType {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "LOCAL" => Ok(Self::Local),
            "FUTURENET" => Ok(Self::Futurenet),
            "TESTNET" => Ok(Self::Testnet),
            "MAINNET" => Ok(Self::Mainnet),
            "CUSTOM" => Ok(Self::Custom),
            other => Err(EngineError::Validation {
                path: "/network/type".to_owned(),
                detail: format!("unrecognised network type {other:?}"),
            }),
        }
    }
}

/// A network descriptor.
///
/// The passphrase is the Stellar fact that identifies a network; the type is an
/// Amasario classification that groups environments sharing operational semantics.
/// They are separate because two environments can share a passphrase while having
/// different endpoints, retention and operators - a private deployment of Stellar
/// core using the public testnet passphrase is a `CUSTOM` network by type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// Stable identifier for this descriptor.
    pub id: String,
    /// The Amasario classification of the environment.
    #[serde(rename = "type")]
    pub network_type: NetworkType,
    /// The Stellar network passphrase.
    pub passphrase: String,
    /// The network id derived from the passphrase, when computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// The RPC endpoint used, recorded because a boundary is only reproducible
    /// when the source of the observation is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    /// The Stellar protocol version observed, where Soroban capabilities are
    /// protocol-versioned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
}

impl Network {
    /// Builds a network descriptor, rejecting an empty passphrase.
    pub fn new(
        id: impl Into<String>,
        network_type: NetworkType,
        passphrase: impl Into<String>,
    ) -> Result<Self> {
        let id = id.into();
        let passphrase = passphrase.into();
        if id.is_empty() {
            return Err(EngineError::Validation {
                path: "/network/id".to_owned(),
                detail: "a network descriptor needs an identifier".to_owned(),
            });
        }
        if passphrase.is_empty() {
            return Err(EngineError::Validation {
                path: "/network/passphrase".to_owned(),
                detail: "a network descriptor needs a passphrase, which is what identifies \
                         the Stellar network"
                    .to_owned(),
            });
        }
        Ok(Self {
            id,
            network_type,
            passphrase,
            network_id: None,
            rpc_url: None,
            protocol_version: None,
        })
    }

    /// Records the RPC endpoint the observation came from.
    #[must_use]
    pub fn with_rpc_url(mut self, url: impl Into<String>) -> Self {
        self.rpc_url = Some(url.into());
        self
    }

    /// Records the observed protocol version.
    #[must_use]
    pub const fn with_protocol_version(mut self, version: u32) -> Self {
        self.protocol_version = Some(version);
        self
    }

    /// Whether two descriptors denote the same Stellar network.
    ///
    /// Compared by passphrase, not by identifier or type: the passphrase is the
    /// fact, and two descriptors that share it denote the same chain even if an
    /// operator named them differently.
    #[must_use]
    pub fn is_same_network_as(&self, other: &Self) -> bool {
        self.passphrase == other.passphrase
    }
}

/// The point at which observations were made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationBoundary {
    /// The network the boundary is expressed against.
    pub network: Network,
    /// The inclusive ledger sequence the observation was bounded by.
    pub ledger: LedgerSequence,
    /// When the observation was made, as an RFC 3339 timestamp.
    pub observed_at: String,
    /// The specification version in force when the observation was made, when it
    /// differs from the document's own version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_version: Option<String>,
}

impl ObservationBoundary {
    /// Builds a boundary.
    pub fn new(network: Network, ledger: LedgerSequence, observed_at: impl Into<String>) -> Self {
        Self {
            network,
            ledger,
            observed_at: observed_at.into(),
            spec_version: None,
        }
    }

    /// Whether two boundaries may be compared as observations of the same facts.
    ///
    /// Requires the same network, and requires the network to be durable enough
    /// that its state cannot have been reset between the two boundaries. A
    /// comparison across a `FUTURENET` reset is not a comparison of one fact
    /// observed twice, and the engine refuses it with a reason rather than
    /// producing a difference that a consumer would read as a change.
    pub fn is_comparable_with(&self, other: &Self) -> Result<()> {
        if !self.network.is_same_network_as(&other.network) {
            return Err(EngineError::Snapshot(format!(
                "cannot compare boundaries on different networks: {} and {}",
                self.network.id, other.network.id
            )));
        }
        if !self.network.network_type.is_durable_enough_to_compare() {
            return Err(EngineError::Snapshot(format!(
                "cannot compare observations on a {} network, whose state may be reset \
                 between observations",
                self.network.network_type
            )));
        }
        Ok(())
    }

    /// Orders two boundaries, rejecting an inverted pair.
    ///
    /// An inverted pair would turn every before/after comparison upside down, which
    /// is why it is an error rather than a swappable detail.
    pub fn order(&self, other: &Self) -> Result<std::cmp::Ordering> {
        self.is_comparable_with(other)?;
        Ok(self.ledger.cmp(&other.ledger))
    }
}

/// Something observed at a boundary.
///
/// Deliberately generic: the kind of thing observed is carried by the payload, and
/// the boundary is not optional. Every observation in the engine is wrapped in one
/// of these, so there is no path by which a fact enters the analysis without the
/// qualification that makes it reproducible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation<T> {
    /// The thing observed.
    pub value: T,
    /// Where and when it was observed.
    pub boundary: ObservationBoundary,
    /// Whether the observation was a direct reading or a derivation from other
    /// observations.
    pub provenance: ObservationProvenance,
}

/// Where an observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservationProvenance {
    /// Read from a network endpoint.
    Network,
    /// Read from local input, such as a supplied WASM file or configuration.
    Local,
    /// Derived from other observations rather than read directly.
    ///
    /// Kept distinct so that a report can separate what was seen from what was
    /// concluded, which is the separation the specification requires of a report's
    /// sections.
    Derived,
}

impl<T> Observation<T> {
    /// Wraps a value observed at a boundary.
    #[must_use]
    pub const fn new(
        value: T,
        boundary: ObservationBoundary,
        provenance: ObservationProvenance,
    ) -> Self {
        Self {
            value,
            boundary,
            provenance,
        }
    }

    /// Whether this observation was read directly rather than derived.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        matches!(
            self.provenance,
            ObservationProvenance::Network | ObservationProvenance::Local
        )
    }

    /// Maps the observed value, preserving the boundary and provenance.
    ///
    /// Used to project a large observation down to a report field without losing
    /// the qualification, which is the failure this type exists to prevent.
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Observation<U> {
        Observation {
            value: f(self.value),
            boundary: self.boundary,
            provenance: self.provenance,
        }
    }

    /// Borrows the observed value.
    ///
    /// Not a `const fn`: the boundary has to be cloned to travel with the
    /// projection, and cloning is not const. Losing the boundary while projecting
    /// would be a worse outcome than losing constness.
    #[must_use]
    pub fn as_ref(&self) -> Observation<&T> {
        Observation {
            value: &self.value,
            boundary: self.boundary.clone(),
            provenance: self.provenance,
        }
    }
}

/// Why a bounded traversal stopped before exhausting what is reachable.
///
/// The enumeration is the specification's, and its existence is the point: a
/// traversal that stopped must say why, because a consumer cannot distinguish a
/// short result from a complete one after the fact. `RateLimited` is listed
/// separately from the other bounds because it is the most common real cause and
/// the one most easily mistaken for a complete result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum TruncationReason {
    /// The configured depth bound was reached.
    MaxDepthReached,
    /// The configured node count bound was reached.
    MaxNodesReached,
    /// Evidence needed to continue was unavailable.
    EvidenceUnavailable,
    /// The observation boundary was reached: continuing would require observing
    /// beyond it.
    BoundaryReached,
    /// A rate limit was applied by an endpoint.
    RateLimited,
    /// The operation was cancelled.
    Cancelled,
}

impl TruncationReason {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxDepthReached => "MAX_DEPTH_REACHED",
            Self::MaxNodesReached => "MAX_NODES_REACHED",
            Self::EvidenceUnavailable => "EVIDENCE_UNAVAILABLE",
            Self::BoundaryReached => "BOUNDARY_REACHED",
            Self::RateLimited => "RATE_LIMITED",
            Self::Cancelled => "CANCELLED",
        }
    }

    /// Whether the reason is transient, in the sense that a later run with the same
    /// inputs could get further.
    ///
    /// A depth or node bound is a deliberate limit, so a later run reaches the same
    /// point unless the bound changes. A rate limit or a cancellation is not, so the
    /// same run may complete. A consumer deciding whether to re-run needs this
    /// distinction, and it is not derivable from the reason's name.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::RateLimited | Self::Cancelled)
    }
}

impl fmt::Display for TruncationReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TruncationReason {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "MAX_DEPTH_REACHED" => Ok(Self::MaxDepthReached),
            "MAX_NODES_REACHED" => Ok(Self::MaxNodesReached),
            "EVIDENCE_UNAVAILABLE" => Ok(Self::EvidenceUnavailable),
            "BOUNDARY_REACHED" => Ok(Self::BoundaryReached),
            "RATE_LIMITED" => Ok(Self::RateLimited),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(EngineError::Validation {
                path: "/truncationReason".to_owned(),
                detail: format!("unrecognised truncation reason {other:?}"),
            }),
        }
    }
}

/// How a traversal ended.
///
/// Paired with [`TruncationReason`] so that "complete" and "bounded" are distinct
/// outcomes rather than a boolean and a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TraversalOutcome {
    /// Every reachable entity was reached within the bounds.
    Complete,
    /// The traversal stopped before exhausting what is reachable.
    Truncated,
}

impl TraversalOutcome {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "COMPLETE",
            Self::Truncated => "TRUNCATED",
        }
    }

    /// Whether the traversal stopped early.
    #[must_use]
    pub const fn is_truncated(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
    const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";

    fn testnet() -> Network {
        Network::new("testnet", NetworkType::Testnet, TESTNET_PASSPHRASE)
            .expect("a well-formed descriptor")
            .with_rpc_url("https://soroban-testnet.stellar.org")
    }

    fn boundary(network: Network, ledger: u32) -> ObservationBoundary {
        ObservationBoundary::new(
            network,
            LedgerSequence::new(ledger).expect("a real ledger"),
            "2026-09-15T00:00:00Z",
        )
    }

    #[test]
    fn a_network_descriptor_requires_an_identifier_and_a_passphrase() {
        Network::new("testnet", NetworkType::Testnet, TESTNET_PASSPHRASE).expect("valid");
        Network::new("", NetworkType::Testnet, TESTNET_PASSPHRASE).expect_err("no identifier");
        Network::new("testnet", NetworkType::Testnet, "").expect_err("no passphrase");
    }

    #[test]
    fn two_descriptors_denote_the_same_network_when_their_passphrases_agree() {
        let a = testnet();
        let b = Network::new("testnet-alias", NetworkType::Custom, TESTNET_PASSPHRASE)
            .expect("valid")
            .with_rpc_url("https://private.example.test");
        // The passphrase is the fact; the identifier and type are classifications,
        // so a differently named descriptor of the same chain is the same network.
        assert!(a.is_same_network_as(&b));

        let mainnet =
            Network::new("mainnet", NetworkType::Mainnet, MAINNET_PASSPHRASE).expect("valid");
        assert!(!a.is_same_network_as(&mainnet));
    }

    #[test]
    fn an_unrecognised_network_type_must_be_confirmed_rather_than_assumed_safe() {
        // The type set is closed in the engine even though the specification's
        // taxonomy is open, because the engine cannot decide whether to allow a
        // mutating operation on a type it does not know.
        NetworkType::from_str("SANDBOX").expect_err("unrecognised types are rejected");
    }

    #[test]
    fn only_local_environments_are_exempt_from_the_mutating_opt_in() {
        for network_type in [
            NetworkType::Local,
            NetworkType::Futurenet,
            NetworkType::Testnet,
            NetworkType::Mainnet,
            NetworkType::Custom,
        ] {
            let expected = network_type != NetworkType::Local;
            assert_eq!(
                network_type.requires_explicit_opt_in_for_mutation(),
                expected,
                "{network_type}"
            );
        }
        // The prohibition follows from this: an unclassified environment is not a
        // safe one, so it is treated as production.
        assert!(NetworkType::Custom.requires_explicit_opt_in_for_mutation());
    }

    #[test]
    fn only_mainnet_observations_have_production_significance() {
        assert!(NetworkType::Mainnet.is_authoritative());
        assert!(!NetworkType::Testnet.is_authoritative());
        assert!(!NetworkType::Local.is_authoritative());
    }

    #[test]
    fn every_network_type_declares_its_durability() {
        for network_type in [
            NetworkType::Local,
            NetworkType::Futurenet,
            NetworkType::Testnet,
            NetworkType::Mainnet,
            NetworkType::Custom,
        ] {
            assert!(!network_type.durability().is_empty(), "{network_type}");
            assert_eq!(
                NetworkType::from_str(network_type.as_str()).expect("round trip"),
                network_type
            );
        }
    }

    #[test]
    fn boundaries_on_different_networks_are_refused() {
        let a = boundary(testnet(), 100);
        let mainnet =
            Network::new("mainnet", NetworkType::Mainnet, MAINNET_PASSPHRASE).expect("valid");
        let b = boundary(mainnet, 200);
        let error = a
            .is_comparable_with(&b)
            .expect_err("different networks are not comparable");
        assert!(error.to_string().contains("different networks"));
    }

    #[test]
    fn a_boundary_that_may_have_been_reset_is_refused_even_on_the_same_network() {
        let futurenet = Network::new(
            "futurenet",
            NetworkType::Futurenet,
            "Test SDF Future Network ; October 2022",
        )
        .expect("valid");
        let a = boundary(futurenet.clone(), 100);
        let b = boundary(futurenet, 200);
        let error = a
            .is_comparable_with(&b)
            .expect_err("a reset-prone environment cannot be compared across observations");
        assert!(error.to_string().contains("reset"));
    }

    #[test]
    fn boundaries_on_a_durable_network_are_comparable_and_ordered_by_ledger() {
        let earlier = boundary(testnet(), 100);
        let later = boundary(testnet(), 200);
        earlier.is_comparable_with(&later).expect("comparable");
        assert_eq!(
            earlier.order(&later).expect("orderable"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            later.order(&earlier).expect("orderable"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            earlier.order(&earlier).expect("orderable"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn an_observation_carries_its_boundary_and_cannot_be_constructed_without_one() {
        let observation = Observation::new(
            42_u32,
            boundary(testnet(), 1234567),
            ObservationProvenance::Network,
        );
        assert_eq!(observation.value, 42);
        assert_eq!(observation.boundary.ledger.get(), 1234567);
        assert!(observation.is_direct());
    }

    #[test]
    fn a_derived_observation_is_distinguishable_from_a_direct_one() {
        let derived =
            Observation::new("x", boundary(testnet(), 10), ObservationProvenance::Derived);
        // A report must be able to separate what was seen from what was concluded.
        assert!(!derived.is_direct());

        let local = Observation::new("y", boundary(testnet(), 10), ObservationProvenance::Local);
        assert!(local.is_direct());
    }

    #[test]
    fn mapping_an_observation_preserves_its_qualification() {
        let observation = Observation::new(
            "0a1b".to_owned(),
            boundary(testnet(), 55),
            ObservationProvenance::Network,
        );
        let projected = observation.map(|value| value.len());
        assert_eq!(projected.value, 4);
        // Losing the boundary while projecting is the failure this type exists to
        // prevent, so the map preserves it rather than rebuilding the wrapper.
        assert_eq!(projected.boundary.ledger.get(), 55);
        assert_eq!(projected.provenance, ObservationProvenance::Network);
    }

    #[test]
    fn a_truncation_reason_round_trips_and_declares_whether_it_is_transient() {
        let reasons = [
            TruncationReason::MaxDepthReached,
            TruncationReason::MaxNodesReached,
            TruncationReason::EvidenceUnavailable,
            TruncationReason::BoundaryReached,
            TruncationReason::RateLimited,
            TruncationReason::Cancelled,
        ];
        for reason in reasons {
            assert_eq!(
                TruncationReason::from_str(reason.as_str()).expect("round trip"),
                reason
            );
        }
        assert!(TruncationReason::RateLimited.is_transient());
        assert!(TruncationReason::Cancelled.is_transient());
        assert!(!TruncationReason::MaxDepthReached.is_transient());
        assert!(!TruncationReason::BoundaryReached.is_transient());
    }

    #[test]
    fn a_rate_limit_is_distinguishable_from_a_depth_bound() {
        // These are the two cases a consumer most easily confuses, and the one
        // where re-running helps is not the one where the bound was deliberate.
        assert_ne!(
            TruncationReason::RateLimited.as_str(),
            TruncationReason::MaxDepthReached.as_str()
        );
        assert!(TruncationReason::RateLimited.is_transient());
        assert!(!TruncationReason::MaxDepthReached.is_transient());
    }

    #[test]
    fn an_unrecognised_truncation_reason_is_rejected() {
        TruncationReason::from_str("TIMED_OUT").expect_err("not a truncation reason");
    }

    #[test]
    fn a_traversal_outcome_is_never_a_boolean_alone() {
        assert_eq!(TraversalOutcome::Complete.as_str(), "COMPLETE");
        assert_eq!(TraversalOutcome::Truncated.as_str(), "TRUNCATED");
        assert!(!TraversalOutcome::Complete.is_truncated());
        assert!(TraversalOutcome::Truncated.is_truncated());
    }

    #[test]
    fn an_observation_round_trips_through_its_serialised_form() {
        let observation = Observation::new(
            7_u32,
            boundary(testnet(), 99),
            ObservationProvenance::Derived,
        );
        let encoded = serde_json::to_string(&observation).expect("serialises");
        let decoded: Observation<u32> = serde_json::from_str(&encoded).expect("deserialises");
        assert_eq!(decoded, observation);
    }

    #[test]
    fn a_network_descriptor_rejects_an_unknown_field_rather_than_ignoring_it() {
        let json = r#"{"id":"testnet","type":"TESTNET","passphrase":"Test SDF Network ; September 2015","secret":"nope"}"#;
        serde_json::from_str::<Network>(json).expect_err("unknown fields are not ignored");
    }
}
