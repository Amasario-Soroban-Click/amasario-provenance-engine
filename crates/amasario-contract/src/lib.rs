//! Soroban contract inspection: identity, module verification, interface decoding,
//! storage and observed invocations.
//!
//! # What this crate is for
//!
//! Everything above the network layer needs one thing from a contract: a record of
//! what was observed about it, qualified by the boundary it was observed at. This
//! crate produces that record and nothing else. It does not decide what a contract
//! depends on, whether its provenance holds, or what a change to it would affect -
//! those are the provenance, dependency and impact layers' jobs, and each of them
//! consumes this crate's output rather than re-reading the network.
//!
//! # The distinctions this crate exists to preserve
//!
//! **An address is not an identity.** A Soroban contract can be upgraded in place,
//! so the same address can execute different code at two ledgers.
//! [`identity::ContractIdentity`] separates the identifying properties - address,
//! network, executable - from the observational ones, and
//! [`identity::ContractIdentity::fingerprint`] excludes the latter so that one
//! contract observed twice does not acquire two identities.
//!
//! **A contradiction is not a failure.** A module whose bytes do not hash to the
//! digest the network records is two facts that cannot both be true. It is reported
//! as an anomaly on an otherwise complete inspection, not raised as an error, so that
//! the one case the engine most needs to report is not the one that aborts the run.
//!
//! **Absence is not unavailability.** A contract that is not deployed, a WASM hash
//! with no code entry, and a request that failed are three different outcomes.
//! [`errors::InspectionFailure`] enumerates them and
//! [`errors::InspectionFailure::is_absence`] answers the question a caller actually
//! asks.
//!
//! **An observation is not a conclusion.** [`identity::Observedness`] marks how a
//! fact was obtained, and [`identity::Observedness::is_evidence`] is the gate the
//! layers above use before citing one.
//!
//! # What this crate does not claim
//!
//! Amasario is not a security scanner. Nothing here returns a verdict about a
//! contract's safety, and parsing a module's framing is not validating it. A module
//! that this crate reads successfully may be malicious, and a module it cannot read
//! may be perfectly safe; the engine reports what it observed and no more.
//!
//! # Example
//!
//! ```no_run
//! use amasario_contract::identity::{ContractExecutableKind, ContractIdentity};
//! use amasario_core::{ContractId, LedgerSequence, Network, NetworkType, Result};
//!
//! # fn main() -> Result<()> {
//! let network = Network::new("testnet", NetworkType::Testnet, "Test SDF Network ; September 2015")?;
//! let contract = ContractId::new("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4")?;
//!
//! // A Stellar Asset Contract has no deployed module, and its identity is complete
//! // without one. A WASM contract's identity is not.
//! let identity = ContractIdentity::new(
//!     contract,
//!     &network,
//!     ContractExecutableKind::StellarAsset,
//!     None,
//!     LedgerSequence::new(1_000)?,
//! )?;
//!
//! println!("{}", identity.network_qualified_id());
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod errors;
pub mod identity;

// Re-exported together because they are used together: a caller working with a
// contract's identity needs the failure model that says what could not be
// established about it, and having to know which module each lives in would be
// friction with no benefit.
pub use errors::{InspectionFailure, describe, first_failure};
pub use identity::{
    CONTRACT_IDENTITY_VERSION, ContractExecutableKind, ContractIdentity, InstanceModification,
    InstanceModificationKind, Observedness, digest_from_hash_bytes,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name
        // it, and a re-export that went missing would be a breaking change discovered
        // downstream rather than here.
        assert_eq!(ContractExecutableKind::Wasm.as_str(), "WASM");
        assert_eq!(Observedness::Observed.as_str(), "OBSERVED");
        assert_eq!(InstanceModificationKind::Deploy.as_str(), "DEPLOY");
        assert_eq!(CONTRACT_IDENTITY_VERSION, "amasario/contract-identity/v1");
    }
}
