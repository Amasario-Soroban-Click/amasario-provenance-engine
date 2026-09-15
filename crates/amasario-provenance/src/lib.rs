//! Provenance analysis: which source tree a deployed artifact came from, and what
//! supports the claim.
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

pub mod errors;
pub mod source;

// Re-exported together because they are used together: a caller recording a source
// needs the failure model that says what could not be established about it, and
// having to know which module each lives in would be friction with no benefit.
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
    }
}
