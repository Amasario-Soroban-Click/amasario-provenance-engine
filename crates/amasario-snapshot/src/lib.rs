//! Snapshots: the persisted record of one analysis, and the difference between two of them.
//!
//! # Why this layer is where determinism is enforced
//!
//! Every other layer in the engine answers a question about a moment. This one decides what
//! is written down, and a written document outlives the network that justified it - so this
//! is the layer where the specification's determinism requirements stop being advice. A
//! snapshot captured twice from the same state must be the same document; a difference
//! between two snapshots must be a difference between two states rather than between two
//! serialisations; and a pair of snapshots that cannot be compared must say so rather than
//! come back empty.
//!
//! * [`normalize`] fixes the ordering of every set and computes the content digest over the
//!   canonical form with the declared volatile fields removed.
//! * [`capture`] assembles a snapshot and derives its identifier from the contract and the
//!   boundary, so re-capturing a state yields the same identifier rather than a new one.
//! * [`store`] writes and reads the whole document, validating on read rather than trusting
//!   a file that this engine may not have produced.
//! * [`compare`] produces the difference, in canonical mode, with a category, a change type
//!   and a reason on every entry.
//!
//! # The two questions a snapshot answers
//!
//! "Has anything changed?" is answered by the content digest, which is computed over the
//! state and not over the capture - so a re-run on an unchanged contract produces an equal
//! digest and an empty diff, which is the case that would otherwise alert on every schedule.
//!
//! "What changed?" is answered by [`compare`], and the answer is deliberately composed of
//! facts rather than of a score: each entry names the entity, the category, whether the
//! change has an established ordering, the values on either side, and why the difference is
//! a difference. `rules/impact/change-impact` gives the reason the reason is mandatory: a
//! diff is the artefact most likely to be consumed without review, so a reviewer needs
//! something to reject an entry on.
//!
//! # What is deliberately not here
//!
//! No storage engine, no database, no index and no retention policy. A snapshot is a file
//! whose name is derived from its identity, and a directory of them is a set of states. A
//! consumer that wants history keeps the files; a consumer that wants a query engine is
//! building something this layer would only duplicate badly.
//!
//! No diffing of diffs. The identifiers are deterministic so that two diffs *can* be
//! compared, and this crate does not itself compare them: the comparison would be over
//! entries rather than over snapshots, and the specification defines the latter.
//!
//! # Layer relationships
//!
//! * [`amasario_contract`] supplies the contract identity a snapshot is about.
//! * [`amasario_provenance`] supplies the executable identity, the provenance chain and the
//!   attestations it carries.
//! * [`amasario_dependency`] and [`amasario_graph`] supply the dependency set and the graph.
//! * [`amasario_impact`] supplies the findings and the change vocabulary a diff classifies
//!   with, so the diff and the impact layer cannot disagree about what a change is.
//! * [`amasario_evidence`] supplies the records, and the basis each one's class supports,
//!   which is how the snapshot's overall confidence is derived rather than asserted.

pub mod capture;
pub mod compare;
pub mod errors;
pub mod normalize;
pub mod store;

pub use capture::{Capture, SNAPSHOT_ID_PREFIX, Snapshot, snapshot_id};
pub use compare::{
    ChangeCategory, ComparisonMode, Diff, DiffEntry, DiffSummary, IncomparableReason, SnapshotRef,
    compare,
};
pub use errors::{SnapshotFailure, first_failure};
pub use normalize::{
    DEFAULT_VOLATILE_FIELDS, DIGESTED_STATE_FIELDS, canonical_json, canonicalise, content_digest,
    dependency_key, entity_key, validate_volatile_fields, verify_content_digest,
};
pub use store::{SNAPSHOT_EXTENSION, file_name, from_json, read, to_json, to_json_pretty, write};
