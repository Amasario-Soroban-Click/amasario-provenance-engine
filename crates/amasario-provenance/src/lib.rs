//! Provenance analysis: which source tree a deployed artifact came from, what
//! produced it, and what supports the claim.
//!
//! # What this crate is for
//!
//! The question a consumer actually asks is narrow and unforgiving: *does this
//! deployed module correspond to that source revision, and what shows it?* This crate
//! assembles the records that answer it and decides what those records support. It
//! performs no network access and reads no contract: callers hand it records, and it
//! reports what they establish.
//!
//! # The distinctions this module set exists to preserve
//!
//! **A claim is not its evidence.** [`SourceProvenance`] refuses to be constructed
//! without cited evidence. A record asserting a digest with nothing behind it is
//! refused at construction rather than at reporting time, so no code path can produce
//! one and no report can contain one.
//!
//! **A branch name is not a revision.** [`Revision`] distinguishes a commit from a
//! branch or tag, and [`Revision::is_full_length`] distinguishes a full digest from an
//! abbreviation. A branch denotes different commits at different times, so it cannot
//! establish which source was built; [`ProvenanceFailure::RevisionNotImmutable`] says
//! so rather than silently accepting it.
//!
//! **Not being able to check is not a refutation.** [`ProvenanceFailure`] separates an
//! unrecorded record from an unverifiable one from a contradicted one, and only the
//! last makes a claim false.
//!
//! **An artifact is its digest.** [`ArtifactIdentity`] identifies an artifact by
//! content: a filename is a build tool's choice and a size can be padded, while the
//! digest is the only property that survives a copy, a rename and a different machine.
//! [`ArtifactIdentity::matches`] therefore compares digests and never names.
//!
//! **A build record says what was recorded, not what happened.** [`BuildProvenance`]
//! requires its toolchain to be named, because a record that omits it cannot reproduce
//! the artifact and would otherwise read as a reproducible build. The recorded
//! environment is [sanitised](build::sanitise_environment): a build environment
//! routinely contains credentials, and copying them into a snapshot or a log is the
//! failure mode this crate is written to avoid.
//!
//! **An upgrade is not a deployment record of origin.** [`DeploymentKind`] separates
//! the transaction that created a contract from the one that replaced its executable,
//! and from not having established which it was. Only the first says where a contract
//! came from; an upgrade says what it currently is, and a record that did not establish
//! the difference must not be read as either.
//!
//! # What this crate does not claim
//!
//! Amasario is not a security scanner. Nothing here is a safety verdict about a
//! contract, its source or its builder.
//!
//! # Example
//!
//! A source record that resolves to one immutable tree:
//!
//! ```
//! use amasario_provenance::{Repository, Revision, SourceProvenance, VcsKind};
//!
//! # fn main() -> amasario_core::Result<()> {
//! let repository = Repository::new("https://github.com/example/token", VcsKind::Git)?;
//! // A full commit digest, not a branch: only an immutable revision says which tree
//! // was built.
//! let revision = Revision::commit("4f2a9c1e8b7d6350a1f4e2c9b8d7a6f5e4c3b2a1")?;
//! let source = SourceProvenance::new(
//!     repository,
//!     revision,
//!     vec!["transaction 9c1f...: the deploy operation's source claim".to_owned()],
//! )?;
//!
//! assert_eq!(source.repository.owner.as_deref(), Some("example"));
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod artifact;
pub mod build;
pub mod deployment;
pub mod errors;
pub mod source;

// Re-exported together because they are used together: a caller recording a source,
// a build and the artifact it produced needs the failure model that says what could
// not be established about them, and having to know which module each lives in would
// be friction with no benefit. The constants travel with them so that a caller can
// name a redaction rather than restate a string.
pub use artifact::{ArtifactIdentity, ArtifactType, DerivationSource, compare_digests};
pub use build::{
    BuildProvenance, EnvironmentVariable, REDACTED, Reproducibility, ReproducibilityStatus,
    Toolchain, is_secret_name, sanitise_environment,
};
pub use deployment::{DeploymentKind, DeploymentProvenance};
pub use errors::{ProvenanceFailure, describe as describe_failure, first_failure};
pub use source::{Repository, Revision, RevisionKind, SourceProvenance, VcsKind};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name
        // it. A re-export that went missing would otherwise be a breaking change
        // discovered downstream rather than here.
        assert_eq!(VcsKind::Git.as_str(), "GIT");
        assert_eq!(RevisionKind::Commit.as_str(), "COMMIT");
        assert_eq!(ArtifactType::Wasm.as_str(), "WASM");
        assert_eq!(ReproducibilityStatus::Reproduced.as_str(), "REPRODUCED");
        assert_eq!(DeploymentKind::Deploy.as_str(), "DEPLOY");
        assert_eq!(REDACTED, "<redacted>");
    }
}
