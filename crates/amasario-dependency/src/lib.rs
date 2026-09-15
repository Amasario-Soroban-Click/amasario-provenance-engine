//! Dependency classification: what an observed relationship is allowed to be called.
//!
//! # What this crate is for
//!
//! The specification forbids inferring a dependency from anything except evidence:
//! not from two projects mentioning each other, not from two contracts existing in the
//! same ecosystem, not from similar metadata. This crate is where that prohibition is
//! enforced, and it does it in steps that can each refuse:
//!
//! 1. [`detector`] turns observations into [`Candidate`]s and reports what it set
//!    aside, so "no dependency found" and "nothing usable was observed" stay distinct.
//! 2. [`classifier`] decides what each candidate is allowed to be called, checking the
//!    class's required evidence, and refuses the rest by name.
//! 3. [`resolver`] merges repeated observations of one edge conservatively and
//!    assembles a [`DependencySet`], recording refusals rather than dropping them.
//!
//! # The distinctions this crate exists to preserve
//!
//! **A basis is not a class.** [`amasario_core::Basis`] says how a fact was obtained;
//! a [`amasario_core::DependencyClass`] says why the subject requires the object.
//! Classification is where the two meet, and where a claim whose evidence does not
//! support its name is refused.
//!
//! **Similarity is not evidence.** Interface inference may support a `CONTRACT`
//! dependency at no more than low confidence - `dependency/contract-dependency`
//! permits exactly that - and nothing else. It cannot reach a `RUNTIME` claim, and it
//! must never appear among a report's observed facts.
//!
//! **A boundary is not a failure.** An entity Amasario cannot inspect is not an entity
//! that failed inspection. That is why `EXTERNAL` exists as a class, why it must
//! record the boundary, and why it can never be reported as verified.
//!
//! # What this crate does not claim
//!
//! Amasario is not a security scanner. A dependency established here says what the
//! subject requires, not whether the object is safe, honest or load-bearing.
//!
//! # Example
//!
//! A cross-contract call observed in a successful transaction is both a `CONTRACT`
//! and a `RUNTIME` dependency:
//!
//! ```
//! use amasario_core::{
//!     Basis, EntityKind, EntityRef, LedgerSequence, Network, NetworkType, ObservationBoundary,
//!     Relationship,
//! };
//! use amasario_dependency::{Candidate, EvidenceRef, classify, resolve};
//!
//! # fn main() -> amasario_core::Result<()> {
//! let boundary = ObservationBoundary {
//!     network: Network::new("testnet", NetworkType::Testnet, "Test SDF Network ; September 2015")?,
//!     ledger: LedgerSequence::new(1_000)?,
//!     observed_at: "2026-09-15T00:00:00Z".to_owned(),
//!     spec_version: None,
//! };
//! let candidate = Candidate::new(
//!     EntityRef::new(EntityKind::Contract, "C-SUBJECT")?,
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
//! let set = resolve(candidate.subject.clone(), Some(boundary), &[candidate], 5)?;
//! assert_eq!(set.len(), 1);
//! assert!(set.unestablished.is_empty());
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod classifier;
pub mod detector;
pub mod errors;
pub mod resolver;

// Re-exported together because they are used together: a caller classifying an
// observation needs the failure model that says why a candidate could not be
// classified, and having to know which module each lives in would be friction with no
// benefit.
pub use classifier::{
    Candidate, Classification, EvidenceRef, classes_for, classify, why_not_a_dependency,
};
pub use detector::{
    DeclaredDependency, DetectionReport, SkipReason, SkippedObservation, detect_from_declared,
    detect_from_invocations, is_transaction_hash, transaction_of,
};
pub use errors::{DependencyFailure, describe as describe_failure, first_failure};
pub use resolver::{Cycle, Dependency, DependencySet, Unestablished, resolve, weakest_status};

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{EntityKind, Relationship};

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        assert_eq!(SkipReason::TopLevelInvocation.as_str(), "TOP_LEVEL_INVOCATION");
        // A consumer should not have to know which module a concept lives in to name
        // it. A re-export that went missing would otherwise be a breaking change
        // discovered downstream rather than here.
        assert!(why_not_a_dependency(Relationship::BuiltFrom).contains("input"));
        assert_eq!(
            classes_for(Relationship::Invocates, EntityKind::Contract).map(<[_]>::len),
            Some(2)
        );
        assert!(classes_for(Relationship::VerifiedBy, EntityKind::Wasm).is_none());
    }
}
