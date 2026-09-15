//! Source provenance: which source tree a claim is about, and whether that claim
//! can establish anything.
//!
//! # The distinction this module exists for
//!
//! A revision is not a revision. `main` and `9f2c1e0a...` are both "revisions" in
//! ordinary speech, and only one of them identifies a source tree: a branch name
//! denotes whatever commit it pointed at when the reader looked, so two people who
//! both "built main" may have built different code, and a record that says `main`
//! cannot be checked against a deployed artifact at all. Tags move for the same
//! reason when a maintainer re-tags.
//!
//! [`RevisionKind`] records which kind was given, and [`SourceProvenance::is_immutable`]
//! answers the question that follows: whether the record can establish which source
//! was built. Nothing in this module refuses a branch revision - recording that
//! someone claimed to have built `main` is a legitimate observation - but the record
//! says what it is, so the verification layer above reaches `PARTIALLY_VERIFIED`
//! rather than `VERIFIED`.
//!
//! # Identity versus observation
//!
//! [`SourceProvenance::identity_digest`] is computed over the repository and the
//! revision only. The retrieval timestamp and the evidence identifiers are
//! deliberately excluded, for the same reason `ContractIdentity` excludes its
//! observation window: a source tree that has not changed has one identity no matter
//! how many times it was fetched.

use std::fmt;
use std::str::FromStr;

use amasario_core::{Digest, EngineError, Result};
use serde::{Deserialize, Serialize};

use crate::errors::ProvenanceFailure;

/// Which version-control system a repository uses.
///
/// Closed and small. The engine records what it was told and does not deduce the
/// system from the URL, because a self-hosted Git server can serve any path and a
/// guess would be recorded as a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VcsKind {
    /// Git.
    Git,
    /// Mercurial.
    Mercurial,
    /// Something else, recorded as given rather than guessed at.
    Other,
}

impl VcsKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Git => "GIT",
            Self::Mercurial => "MERCURIAL",
            Self::Other => "OTHER",
        }
    }
}

impl fmt::Display for VcsKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for VcsKind {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "GIT" => Ok(Self::Git),
            "MERCURIAL" => Ok(Self::Mercurial),
            "OTHER" => Ok(Self::Other),
            other => Err(EngineError::Validation {
                path: "/vcs".to_owned(),
                detail: format!("unrecognised version-control kind {other:?}"),
            }),
        }
    }
}

/// What kind of revision a record names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RevisionKind {
    /// A commit identifier, which denotes one immutable tree.
    Commit,
    /// A branch name, which denotes different commits at different times.
    Branch,
    /// A tag name, which *should* denote one commit and can be moved.
    Tag,
}

impl RevisionKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "COMMIT",
            Self::Branch => "BRANCH",
            Self::Tag => "TAG",
        }
    }

    /// Whether a revision of this kind identifies one immutable source tree.
    ///
    /// Only a commit does. A tag is mutable in practice - maintainers re-tag - and a
    /// branch is mutable by definition; treating either as immutable would let a
    /// record that cannot be checked be reported as verified.
    #[must_use]
    pub const fn is_immutable(self) -> bool {
        matches!(self, Self::Commit)
    }
}

impl fmt::Display for RevisionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RevisionKind {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "COMMIT" => Ok(Self::Commit),
            "BRANCH" => Ok(Self::Branch),
            "TAG" => Ok(Self::Tag),
            other => Err(EngineError::Validation {
                path: "/revision/kind".to_owned(),
                detail: format!("unrecognised revision kind {other:?}"),
            }),
        }
    }
}

/// A revision of a source repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    /// Whether this is a commit, a branch or a tag.
    pub kind: RevisionKind,
    /// The revision as recorded.
    pub value: String,
    /// The commit the revision resolved to, when the resolver recorded one.
    ///
    /// This is what makes a branch revision usable: a record saying "branch `main`,
    /// which resolved to commit `9f2c…`" *does* identify a source tree, because the
    /// resolution was observed. It is kept separate from the revision rather than
    /// replacing it, so that the record still shows what was asked for and what was
    /// found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
}

/// The shortest and longest hexadecimal commit identifier the engine accepts.
///
/// Forty is a full SHA-1; sixty-four is a full SHA-256. Anything between the two is
/// an abbreviated identifier, which the engine accepts for a *recorded* revision but
/// never invents: an abbreviated identifier is ambiguous in principle, and a record
/// that carries one cannot be checked against a rebuild without resolving it first.
const MIN_COMMIT_HEX: usize = 7;
const FULL_SHA1_HEX: usize = 40;
const FULL_SHA256_HEX: usize = 64;

impl Revision {
    /// A commit revision.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the value is not a hexadecimal commit
    /// identifier of an accepted length.
    pub fn commit(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        Self::validate_commit(&value)?;
        Ok(Self {
            kind: RevisionKind::Commit,
            value,
            resolved_commit: None,
        })
    }

    /// A branch revision.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the name is empty or contains whitespace.
    pub fn branch(value: impl Into<String>) -> Result<Self> {
        Self::named(RevisionKind::Branch, value)
    }

    /// A tag revision.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the name is empty or contains whitespace.
    pub fn tag(value: impl Into<String>) -> Result<Self> {
        Self::named(RevisionKind::Tag, value)
    }

    /// Builds a named revision, validating its shape.
    fn named(kind: RevisionKind, value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProvenanceFailure::RevisionMalformed {
                revision: value,
                detail: "a revision name may not be empty".to_owned(),
            }
            .into_error());
        }
        if value.chars().any(char::is_whitespace) {
            return Err(ProvenanceFailure::RevisionMalformed {
                revision: value,
                detail: "a revision name may not contain whitespace".to_owned(),
            }
            .into_error());
        }
        Ok(Self {
            kind,
            value,
            resolved_commit: None,
        })
    }

    /// Checks that a commit identifier has the form of one.
    fn validate_commit(value: &str) -> Result<()> {
        if value.len() < MIN_COMMIT_HEX || value.len() > FULL_SHA256_HEX {
            return Err(ProvenanceFailure::RevisionMalformed {
                revision: value.to_owned(),
                detail: format!(
                    "a commit identifier is between {MIN_COMMIT_HEX} and {FULL_SHA256_HEX} \
                     hexadecimal characters, but this one is {}",
                    value.len()
                ),
            }
            .into_error());
        }
        if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ProvenanceFailure::RevisionMalformed {
                revision: value.to_owned(),
                detail: "a commit identifier is hexadecimal".to_owned(),
            }
            .into_error());
        }
        // Lengths between a full SHA-1 and a full SHA-256 are possible for an
        // abbreviated SHA-256 and are accepted; the check above already bounded the
        // range, and rejecting a length the engine cannot interpret would be a
        // different decision than the one made here.
        Ok(())
    }

    /// Records the commit a named revision resolved to.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the recorded value is not a commit
    /// identifier, which would mean a resolution to something that is not one.
    pub fn resolved_to(mut self, commit: impl Into<String>) -> Result<Self> {
        let commit = commit.into();
        Self::validate_commit(&commit)?;
        self.resolved_commit = Some(commit);
        Ok(self)
    }

    /// Whether this revision identifies one immutable source tree.
    ///
    /// A commit does; a named revision does once it has been resolved to a commit;
    /// nothing else does.
    #[must_use]
    pub const fn is_immutable(&self) -> bool {
        self.kind.is_immutable() || self.resolved_commit.is_some()
    }

    /// The commit that identifies the source tree, when there is one.
    #[must_use]
    pub fn commit_id(&self) -> Option<&str> {
        match self.kind {
            RevisionKind::Commit => Some(&self.value),
            RevisionKind::Branch | RevisionKind::Tag => self.resolved_commit.as_deref(),
        }
    }

    /// Whether the recorded value is a full-length identifier.
    ///
    /// A record carrying an abbreviated commit can still be displayed, but comparing
    /// it against a rebuild requires resolving it, so the engine reports the
    /// distinction rather than assuming the abbreviation is unambiguous.
    #[must_use]
    pub const fn is_full_length(&self) -> bool {
        matches!(self.value.len(), FULL_SHA1_HEX | FULL_SHA256_HEX)
    }
}

/// A source repository, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    /// The canonical URL the repository was retrieved from.
    pub url: String,
    /// The version-control system.
    pub vcs: VcsKind,
    /// The account or organisation the repository belongs to, when the URL makes it
    /// unambiguous.
    ///
    /// Recorded rather than derived at each use: deriving an owner from a URL means
    /// parsing each provider's path convention, and doing that in two places is how
    /// two answers come to disagree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// The URL schemes the engine accepts for a repository.
///
/// An allow-list rather than a deny-list. A record naming a scheme outside this set
/// is not necessarily wrong, but the engine cannot tell a repository URL from a URL
/// that would perform a request when fetched, and the specification forbids the
/// engine from being an instrument for that.
const ACCEPTED_SCHEMES: &[&str] = &["https", "http", "ssh", "git", "file"];

impl Repository {
    /// Validates and records a repository.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the URL has no scheme, has a scheme outside
    /// the accepted set, or has no host where one is required.
    pub fn new(url: impl Into<String>, vcs: VcsKind) -> Result<Self> {
        let url = url.into();
        let parsed = url::Url::parse(&url).map_err(|error| {
            ProvenanceFailure::RepositoryMalformed {
                url: url.clone(),
                detail: error.to_string(),
            }
            .into_error()
        })?;

        let scheme = parsed.scheme().to_ascii_lowercase();
        if !ACCEPTED_SCHEMES.contains(&scheme.as_str()) {
            return Err(ProvenanceFailure::RepositoryMalformed {
                url,
                detail: format!(
                    "the scheme {scheme:?} is not one the engine reads repositories from; accepted \
                     schemes are {}",
                    ACCEPTED_SCHEMES.join(", ")
                ),
            }
            .into_error());
        }
        // An empty host is checked as well as an absent one: `https:///repo` parses
        // successfully with a present-but-empty host, so testing only for absence
        // would accept a URL that names no repository anywhere and then record it as
        // the identity of a source.
        if scheme != "file" && parsed.host_str().is_none_or(str::is_empty) {
            return Err(ProvenanceFailure::RepositoryMalformed {
                url,
                detail: "a repository URL over a network scheme needs a non-empty host".to_owned(),
            }
            .into_error());
        }

        let owner = parsed
            .path_segments()
            .and_then(|mut segments| segments.next())
            .filter(|segment| !segment.is_empty())
            .map(ToOwned::to_owned);

        Ok(Self { url, vcs, owner })
    }

    /// The repository path with any scheme and host removed.
    ///
    /// Used for display and for grouping, never for identity: the full URL is the
    /// identity, because two hosts can serve a path of the same name.
    #[must_use]
    pub fn path(&self) -> String {
        url::Url::parse(&self.url).map_or_else(
            |_| self.url.clone(),
            |parsed| parsed.path().trim_start_matches('/').to_owned(),
        )
    }
}

/// Source provenance for one entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    /// The repository.
    pub repository: Repository,
    /// The revision.
    pub revision: Revision,
    /// The path within the repository, for a monorepo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    /// The digest of a source archive, when one was retrieved.
    ///
    /// Separate from the revision because the two answer different questions: the
    /// revision says which tree, and the archive digest says that the bytes in hand
    /// are that tree. A record with a revision and no archive digest can say which
    /// source *should* have been retrieved but not that it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_digest: Option<Digest>,
    /// When the source was retrieved, as an RFC 3339 timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<String>,
    /// Identifiers of the evidence records that support this claim. Never empty.
    pub evidence: Vec<String>,
}

impl SourceProvenance {
    /// Records source provenance.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when no evidence is cited. A source claim with
    /// nothing behind it is exactly what the specification forbids, and it is
    /// refused here rather than at the reporting layer so that no code path can
    /// construct one.
    pub fn new(repository: Repository, revision: Revision, evidence: Vec<String>) -> Result<Self> {
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/sourceProvenance/evidence".to_owned(),
                detail: "a source claim must cite the evidence that supports it; a record with no \
                         evidence asserts a source without naming anything that shows it"
                    .to_owned(),
            });
        }
        Ok(Self {
            repository,
            revision,
            subdirectory: None,
            archive_digest: None,
            retrieved_at: None,
            evidence,
        })
    }

    /// Records a monorepo subdirectory.
    #[must_use]
    pub fn in_subdirectory(mut self, path: impl Into<String>) -> Self {
        self.subdirectory = Some(path.into());
        self
    }

    /// Records the digest of the retrieved source archive.
    #[must_use]
    pub fn with_archive_digest(mut self, digest: Digest) -> Self {
        self.archive_digest = Some(digest);
        self
    }

    /// Records when the source was retrieved.
    #[must_use]
    pub fn retrieved_at(mut self, timestamp: impl Into<String>) -> Self {
        self.retrieved_at = Some(timestamp.into());
        self
    }

    /// Whether this record can establish which source tree was built.
    ///
    /// Requires a revision that resolves to a commit. The archive digest is not
    /// required: a commit identifier is enough to determine the tree, and demanding
    /// the archive as well would reject records that are entirely adequate.
    #[must_use]
    pub const fn is_immutable(&self) -> bool {
        self.revision.is_immutable()
    }

    /// A digest over the repository and revision only.
    ///
    /// Excludes the retrieval time and the evidence identifiers so that one source
    /// tree has one identity. The subdirectory and the archive digest are included
    /// because they select a *different* tree: two contracts built from two
    /// directories of one repository are two sources, and the same revision fetched
    /// twice must digest to two different archives only if the bytes differed, which
    /// is what a digest comparison already establishes.
    #[must_use]
    pub fn identity_digest(&self) -> Digest {
        let mut canonical = String::with_capacity(256);
        canonical.push_str("amasario/source\n");
        canonical.push_str(&self.repository.url);
        canonical.push('\n');
        canonical.push_str(self.repository.vcs.as_str());
        canonical.push('\n');
        canonical.push_str(self.revision.kind.as_str());
        canonical.push('\n');
        canonical.push_str(&self.revision.value);
        canonical.push('\n');
        canonical.push_str(self.revision.resolved_commit.as_deref().unwrap_or("-"));
        canonical.push('\n');
        canonical.push_str(self.subdirectory.as_deref().unwrap_or("-"));
        canonical.push('\n');
        canonical.push_str(self.archive_digest.as_ref().map_or("-", Digest::value));
        canonical.push('\n');
        Digest::sha256_of(canonical.as_bytes())
    }

    /// Whether two records describe the same source.
    #[must_use]
    pub fn is_same_source_as(&self, other: &Self) -> bool {
        self.identity_digest().matches(&other.identity_digest())
    }

    /// Checks that the record's own invariants hold.
    ///
    /// A record can arrive by deserialisation rather than through the constructor, so
    /// the evidence requirement is restated as a check.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when no evidence is cited.
    pub fn validate(&self) -> Result<()> {
        if self.evidence.is_empty() {
            return Err(ProvenanceFailure::SourceUnrecorded {
                subject: self.repository.url.clone(),
            }
            .into_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1: &str = "9f2c1e0a3b4c5d6e7f8091a2b3c4d5e6f7081920";

    fn repository() -> Repository {
        Repository::new("https://github.com/example/repo", VcsKind::Git).expect("a valid URL")
    }

    #[test]
    fn a_commit_revision_identifies_a_source_tree() {
        let revision = Revision::commit(SHA1).expect("a valid commit");
        assert!(revision.is_immutable());
        assert_eq!(revision.commit_id(), Some(SHA1));
        assert!(revision.is_full_length());
        assert_eq!(revision.kind, RevisionKind::Commit);
    }

    #[test]
    fn a_branch_revision_does_not_identify_a_source_tree_until_it_resolves() {
        // The distinction the module exists for: two people who both "built main"
        // may have built different code.
        let branch = Revision::branch("main").expect("a valid name");
        assert!(!branch.is_immutable());
        assert_eq!(branch.commit_id(), None);

        let resolved = branch.resolved_to(SHA1).expect("a valid commit");
        assert!(resolved.is_immutable());
        assert_eq!(resolved.commit_id(), Some(SHA1));
        assert_eq!(
            resolved.value, "main",
            "the record still shows what was asked for"
        );
        assert_eq!(resolved.resolved_commit.as_deref(), Some(SHA1));
    }

    #[test]
    fn a_tag_is_not_treated_as_immutable() {
        // Maintainers re-tag, so a tag name does not identify one tree.
        let tag = Revision::tag("v1.0.0").expect("a valid name");
        assert!(!tag.is_immutable());
        assert!(!RevisionKind::Tag.is_immutable());
        assert!(!RevisionKind::Branch.is_immutable());
        assert!(RevisionKind::Commit.is_immutable());
    }

    #[test]
    fn a_commit_revision_must_be_hexadecimal_of_an_accepted_length() {
        Revision::commit(SHA1).expect("forty hex characters");
        Revision::commit("a".repeat(64)).expect("a full SHA-256");
        Revision::commit("a".repeat(7)).expect("the shortest abbreviation");
        Revision::commit("a".repeat(6)).expect_err("too short to identify anything");
        Revision::commit("a".repeat(65)).expect_err("longer than a SHA-256");
        Revision::commit("z".repeat(40)).expect_err("not hexadecimal");
    }

    #[test]
    fn an_abbreviated_commit_is_recorded_as_abbreviated() {
        // It can be displayed but cannot be compared against a rebuild without being
        // resolved, so the record says which it is.
        let abbreviated = Revision::commit("9f2c1e0").expect("seven characters");
        assert!(!abbreviated.is_full_length());
        assert!(Revision::commit(SHA1).expect("valid").is_full_length());
    }

    #[test]
    fn a_named_revision_must_be_a_usable_name() {
        Revision::branch("").expect_err("an empty name denotes nothing");
        Revision::branch("feature branch").expect_err("whitespace is not part of a name");
        Revision::tag("v1.0.0").expect("a valid tag");
    }

    #[test]
    fn a_resolution_to_something_that_is_not_a_commit_is_rejected() {
        let branch = Revision::branch("main").expect("valid");
        branch
            .clone()
            .resolved_to("not-a-commit")
            .expect_err("a resolution must name a commit");
        branch
            .resolved_to("zz")
            .expect_err("a two-character value is not a commit");
    }

    #[test]
    fn revision_kinds_round_trip_through_their_wire_names() {
        for kind in [
            RevisionKind::Commit,
            RevisionKind::Branch,
            RevisionKind::Tag,
        ] {
            assert_eq!(
                RevisionKind::from_str(kind.as_str()).expect("round trip"),
                kind
            );
        }
        assert_eq!(VcsKind::from_str("GIT").expect("round trip"), VcsKind::Git);
        RevisionKind::from_str("SNAPSHOT").expect_err("an unknown kind is rejected");
        VcsKind::from_str("BZR").expect_err("an unknown kind is rejected");
    }

    #[test]
    fn a_repository_url_must_have_an_accepted_scheme() {
        Repository::new("https://github.com/example/repo", VcsKind::Git).expect("https");
        Repository::new("ssh://git@example.invalid/repo", VcsKind::Git).expect("ssh");
        Repository::new("file:///srv/git/repo", VcsKind::Git).expect("a local path");
        Repository::new("ftps://example.invalid/repo", VcsKind::Git)
            .expect_err("an unreadable scheme is refused");
        Repository::new("not a url at all", VcsKind::Git).expect_err("no scheme");
    }

    #[test]
    fn a_network_repository_url_without_a_host_is_refused() {
        // `ssh` is not one of the WHATWG "special" schemes, so an empty host parses
        // and would otherwise be recorded as a source that names no host anywhere.
        Repository::new("ssh:///repo", VcsKind::Git).expect_err("an empty host names nothing");
        // A special scheme never reaches that check, because the parser refuses an
        // empty host for it outright.
        Repository::new("https://", VcsKind::Git).expect_err("the parser refuses an empty host");
    }

    #[test]
    fn extra_slashes_before_the_host_do_not_produce_a_hostless_url() {
        // Worth pinning down because it is the opposite of what the shape suggests:
        // WHATWG parsing skips the leading slashes after the scheme, so `https:///repo`
        // names the host `repo` with an empty path rather than naming no host. The
        // engine accepts it on that basis, and this test states the basis rather than
        // leaving the next reader to guess.
        let repository = Repository::new("https:///repo", VcsKind::Git).expect("a host");
        assert_eq!(repository.url, "https:///repo");
        assert_eq!(repository.path(), "");
        assert_eq!(repository.owner, None);
    }

    #[test]
    fn the_repository_owner_is_read_from_the_path_once() {
        let repository = repository();
        assert_eq!(repository.owner.as_deref(), Some("example"));
        assert_eq!(repository.path(), "example/repo");
    }

    #[test]
    fn a_source_claim_must_cite_evidence() {
        // A record with no evidence asserts a source without naming anything that
        // shows it, which is the shape of claim the specification forbids.
        let error = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            Vec::new(),
        )
        .expect_err("no evidence");
        assert!(error.to_string().contains("must cite the evidence"));

        SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["evidence-1".to_owned()],
        )
        .expect("one citation is enough");
    }

    #[test]
    fn the_identity_excludes_the_retrieval_time_and_the_citations() {
        // One source tree has one identity however many times it is fetched.
        let base = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["evidence-1".to_owned()],
        )
        .expect("valid")
        .retrieved_at("2026-09-15T00:00:00Z");

        let again = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["evidence-2".to_owned(), "evidence-3".to_owned()],
        )
        .expect("valid")
        .retrieved_at("2027-01-01T12:00:00Z");

        assert!(base.is_same_source_as(&again));
        assert_eq!(base.identity_digest(), again.identity_digest());
    }

    #[test]
    fn the_identity_distinguishes_a_subdirectory_and_an_archive() {
        // Two contracts built from two directories of one repository are two sources.
        let root = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["e".to_owned()],
        )
        .expect("valid");
        let nested = root.clone().in_subdirectory("contracts/token");
        assert!(!root.is_same_source_as(&nested));

        let archived = root
            .clone()
            .with_archive_digest(Digest::sha256_of(b"archive"));
        assert!(!root.is_same_source_as(&archived));
    }

    #[test]
    fn a_branch_record_and_a_commit_record_are_different_sources() {
        // A branch that resolved to a commit is a different record from the commit
        // itself, and the two must not be conflated: one says which tree was built
        // and the other says that a branch pointed there when it was read.
        let commit = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["e".to_owned()],
        )
        .expect("valid");
        let branch = SourceProvenance::new(
            repository(),
            Revision::branch("main")
                .expect("valid")
                .resolved_to(SHA1)
                .expect("valid"),
            vec!["e".to_owned()],
        )
        .expect("valid");

        assert_ne!(commit.identity_digest(), branch.identity_digest());
        assert!(commit.is_immutable());
        assert!(branch.is_immutable());
    }

    #[test]
    fn a_deserialised_record_without_evidence_is_caught_by_validation() {
        let mut record = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["e".to_owned()],
        )
        .expect("valid");
        record.validate().expect("self-consistent");
        record.evidence.clear();
        record
            .validate()
            .expect_err("a record with no evidence is not a record");
    }

    #[test]
    fn source_provenance_round_trips_through_json() {
        let original = SourceProvenance::new(
            repository(),
            Revision::commit(SHA1).expect("valid"),
            vec!["e".to_owned()],
        )
        .expect("valid")
        .in_subdirectory("contracts/token");
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: SourceProvenance = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, original);
        restored.validate().expect("self-consistent");
    }
}
