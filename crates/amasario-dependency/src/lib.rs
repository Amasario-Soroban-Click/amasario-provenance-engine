//! Dependency discovery and classification for Soroban contracts.
//!
//! # What this crate is for
//!
//! The specification forbids inferring a dependency from anything except evidence:
//! not from two projects mentioning each other, not from two contracts existing in the
//! same ecosystem, not from similar metadata. This crate is where that prohibition is
//! enforced, and it does it in four steps, each of which can refuse:
//!
//! 1. [`detector`] turns observations into [`Candidate`]s and reports what it set
//!    aside, so "no dependency found" and "nothing usable was observed" stay distinct.
//! 2. [`classifier`] decides what each candidate is allowed to be called, checking the
//!    class's required evidence, and refuses the rest by name.
//! 3. [`resolver`] merges repeated observations of one edge conservatively and
//!    assembles a [`DependencySet`], recording refusals rather than dropping them.
//! 4. [`direct`] and [`transitive`] partition that set, and [`transitive`] closes it
//!    under a bounded traversal that reports truncation and cycles instead of hiding
//!    them.
//!
//! # The distinctions this crate exists to preserve
//!
//! **A basis is not a class.** [`amasario_core::Basis`] says how a fact was obtained;
//! a [`amasario_core::DependencyClass`] says why the subject requires the object.
//! Classification is
//! where the two meet, and where a claim whose evidence does not support its name is
//! refused.
//!
//! **Similarity is not evidence.** Interface inference may support a `CONTRACT`
//! dependency at no more than low confidence - `dependency/contract-dependency`
//! permits exactly that - and nothing else. It cannot reach a `RUNTIME` claim, and it
//! must never appear among a report's observed facts.
//!
//! **An artifact dependency rests on a digest.** `dependency/artifact-dependency`
//! requires a digest match or a declared build input, because size, media type, module
//! name and version string are the signals that coincide by chance and leave nothing
//! to re-compute.
//!
//! **A boundary is not a failure.** An entity Amasario cannot inspect is not an entity
//! that failed inspection. That is why `EXTERNAL` exists as a class, why it must
//! record the boundary, and why it can never be reported as verified.
//!
//! **A bounded search says it was bounded.** A truncated set carries its reason, and
//! a cycle is reported with the edges that form it rather than removed.
//!
//! # What this crate does not claim
//!
//! Amasario is not a security scanner. A dependency established here says what the
//! subject requires, not whether the object is safe, honest or load-bearing.
//!
//! # Example
//!
//! A cross-contract call observed in a successful transaction is both a `CONTRACT`
//! and a `RUNTIME` dependency; the same call in a reverted transaction is neither:
//!
//! ```
//! use amasario_core::{
//!     Basis, EntityKind, EntityRef, LedgerSequence, Network, NetworkType, ObservationBoundary,
//!     Relationship,
//! };
//! use amasario_dependency::{EvidenceRef, Candidate, classify, resolve};
//!
//! # fn main() -> amasario_core::Result<()> {
//! let boundary = ObservationBoundary {
//!     network: Network::new("testnet", NetworkType::Testnet, "Test SDF Network ; September 2015")?,
//!     ledger: LedgerSequence::new(1_000)?,
//!     observed_at: "2026-09-15T00:00:00Z".to_owned(),
//!     spec_version: None,
//! };
//! let subject = EntityRef::new(EntityKind::Contract, "C-SUBJECT")?;
//! let candidate = Candidate::new(
//!     subject.clone(),
//!     EntityRef::new(EntityKind::Contract, "C-CALLEE")?,
//!     Relationship::Invocates,
//!     Basis::ObservedInvocation,
//!     vec![EvidenceRef::new(amasario_core::EvidenceType::Transaction, "ab".repeat(32))?],
//! )?
//! .observed_at(boundary.clone())
//! .with_outcome(Some(true));
//!
//! let classification = classify(&candidate)?;
//! assert!(classification.is_observed());
//!
//! let set = resolve(subject, Some(boundary), &[candidate], 5)?;
//! assert_eq!(set.len(), 1);
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod artifact;
pub mod classifier;
pub mod contract;
pub mod detector;
pub mod direct;
pub mod errors;
pub mod resolver;
pub mod transitive;

// Re-exported together because they are used together: a caller detecting
// observations is also the caller classifying them and assembling a set, and having
// to know which module each name lives in would be friction with no benefit. The
// constants travel with them so that a caller can name a bound rather than restate a
// number.
pub use artifact::{
    ArtifactDependencyOutcome, establish_external, establish_from_digest, is_artifact_target,
};
pub use classifier::{
    Candidate, Classification, EvidenceRef, classes_for, classify, why_not_a_dependency,
};
pub use contract::{
    ContractDependency, contract_dependencies, networks, runtime_dependencies, unresolvable,
};
pub use detector::{
    DeclaredDependency, DetectionReport, SkipReason, SkippedObservation, detect_from_declared,
    detect_from_invocations, is_transaction_hash, transaction_of,
};
pub use direct::{
    DirectSummary, assert_partition_is_sound, direct_dependencies, direct_targets, is_direct,
    observed_direct, summarise_direct,
};
pub use errors::{DependencyFailure, describe as describe_failure, first_failure};
pub use resolver::{Cycle, Dependency, DependencySet, Unestablished, resolve, weakest_status};
pub use transitive::{
    Closure, DEFAULT_MAX_DEPTH, DEFAULT_MAX_NODES, EdgeSource, Limits, apply, close, close_set,
    uniform_relationship,
};

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
        ObservationBoundary, Relationship,
    };

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(1_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name
        // it. A re-export that went missing would otherwise be a breaking change
        // discovered downstream rather than here.
        assert_eq!(
            SkipReason::TopLevelInvocation.as_str(),
            "TOP_LEVEL_INVOCATION"
        );
        assert!(why_not_a_dependency(Relationship::BuiltFrom).contains("input"));
        assert_eq!(
            classes_for(Relationship::Invocates, EntityKind::Contract).map(<[_]>::len),
            Some(2)
        );
        assert!(classes_for(Relationship::VerifiedBy, EntityKind::Wasm).is_none());
    }

    #[test]
    fn a_cross_contract_call_is_classified_and_resolved_through_the_crate_root() {
        let subject = EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference");
        let candidate = Candidate::new(
            subject.clone(),
            EntityRef::new(EntityKind::Contract, "C-callee").expect("a reference"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true));

        let set = resolve(subject, Some(boundary()), &[candidate], 5).expect("resolves");
        assert_eq!(set.len(), 1);
        assert!(set.unestablished.is_empty());
        assert!(
            set.with_class(amasario_core::DependencyClass::Runtime)
                .len()
                == 1
        );
    }
}
