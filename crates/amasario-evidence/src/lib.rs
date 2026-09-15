//! Evidence: the only thing that makes a claim assertable.
//!
//! # Why this layer exists at all
//!
//! The specification is explicit that "evidence is the only thing that makes a claim
//! assertable", and equally explicit that confidence "must never replace evidence".
//! Together those statements fix this crate's responsibility: collect the records a run
//! actually gathered, check each against what its class requires, and report what those
//! records support. It does not decide what ought to be true about a contract, and it
//! cannot be asked for a conclusion without producing the records the conclusion rests on.
//!
//! # What a record carries
//!
//! [`collector::EvidenceRecord`] mirrors `schema/evidence.schema.json` field for field,
//! including the per-class fields only some classes use: `repository` and `revision` for
//! source evidence, `toolchain` and `configuration_digest` for build evidence, `digest` and
//! `artifact_type` for artifact and wasm evidence, `event_index` for events, `successful`
//! for transactions, `attestation_id` for attestations and `observation_note` for
//! observations. One type serving nine classes keeps the collected set homogeneous - a
//! report serialises it with one shape - while [`collector::EvidenceRecord::failures`]
//! enforces the per-class requirements that make the uniformity safe rather than sloppy.
//!
//! # Traceability is implemented, not asserted
//!
//! Each class declares what its records must be traceable to in
//! `taxonomies/evidence-types.yaml`, and that declaration is enforced here:
//!
//! | Class | The record must carry | Built by |
//! | --- | --- | --- |
//! | `SOURCE` | a repository and an immutable revision | [`source::from_source`] |
//! | `BUILD` | a toolchain identity and a configuration digest | [`source::from_build`] |
//! | `ARTIFACT` | a digest and the artifact class it was taken over | [`artifact::from_artifact`] |
//! | `WASM` | an executable digest and the boundary the network reported it at | [`artifact::from_executable`] |
//! | `TRANSACTION` | a hash, a boundary and a stated outcome | [`transaction::from_outcome`] |
//! | `DEPLOYMENT` | the transaction and ledger that recorded the transition | [`deployment::from_deployment`] |
//! | `EVENT` | the emitting transaction and the event's index within it | [`event::from_event`] |
//! | `ATTESTATION` | the attestation's identifier and the claim it states | [`verifier::from_attestation`] |
//! | `OBSERVATION` | what was observed, stated factually | [`collector::EvidenceRecord::new`] |
//!
//! A class with no constructor of its own is not an omission: `WASM` is built by the same
//! function as `ARTIFACT` because the difference between them is which content the digest
//! addresses, and `OBSERVATION` has nothing class-specific to set beyond its note.
//!
//! # Unrecognised classes are kept and inert
//!
//! `taxonomies/evidence-types.yaml` is `open` with `consumersMustHandleUnknown: true`. A
//! record whose class this version does not define is stored with its raw term, supports
//! nothing ([`collector::EvidenceRecord::supports_claim`] is `false`), and is rated
//! `UNKNOWN` by [`confidence`]. Discarding it would lose a record a later version can
//! interpret, and interpreting it would be the guess the open vocabulary forbids.
//!
//! # Layer relationships
//!
//! * [`amasario_core`] supplies the identities and the observation boundary every record is
//!   anchored to, plus the confidence and verification vocabularies.
//! * [`amasario_provenance`] supplies the source, build, artifact, deployment and
//!   attestation models the constructors consume, so this crate does not restate them.
//! * [`amasario_dependency`] consumes the [`amasario_dependency::EvidenceRef`] this crate
//!   produces, which is how an edge cites evidence instead of describing it.
//!
//! # Guarantees
//!
//! * Every constructor returns a validated record. The only way to hold an unvalidated one
//!   is [`collector::EvidenceRecord::draft`], and
//!   [`collector::EvidenceRegistry::add`] re-checks before accepting.
//! * Two records in one registry cannot share an identifier, because claims cite records by
//!   identifier and a duplicate would make every citation ambiguous.
//! * A failed transaction is recorded as an attempt and refused for any claim about its
//!   effect ([`transaction`], [`deployment`]).
//! * Collection order never changes the set: [`collector::EvidenceRegistry`] keeps records
//!   in a deterministic order regardless of arrival order.

pub mod artifact;
pub mod collector;
pub mod confidence;
pub mod deployment;
pub mod errors;
pub mod event;
pub mod source;
pub mod transaction;
pub mod verifier;
