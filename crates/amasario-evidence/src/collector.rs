//! The evidence record, and the set of records an analysis collected.
//!
//! # A record is about a claim, not about a thing
//!
//! `schema/evidence.schema.json` requires `id`, `type`, `claim` and `observedAt`, and the
//! taxonomy says why the claim is mandatory: evidence "is a record about a claim, never a
//! property of an artifact". A digest is not evidence on its own; the record is the
//! statement "these bytes hash to this value, which supports this claim", and the claim
//! is what a reader checks the record against.
//!
//! # Unrecognised classes are stored and inert
//!
//! `taxonomies/evidence-types.yaml` is `open` with `consumersMustHandleUnknown: true`, and
//! the schema states the consequence: an unrecognised term "requires the consumer to treat
//! the evidence as UNKNOWN rather than to" reject it. [`EvidenceClass`] therefore has a
//! variant that keeps the raw term, [`EvidenceRecord::supports_claim`] returns `false`
//! for it, and [`crate::confidence`] rates it `UNKNOWN`. Nothing is discarded and nothing
//! is interpreted - which is what treating it as unknown means, and the only reading that
//! is honest at a version that does not know the term.
//!
//! # Why the registry refuses a duplicate identifier
//!
//! Claims reference evidence by identifier, so two records sharing one would make every
//! citation of it ambiguous between them. The registry refuses the second rather than
//! keeping both, because the ambiguity is created at collection time and cannot be
//! resolved later: once a dependency edge cites `e-3`, nothing in the record says which
//! `e-3` it meant.

use amasario_core::{
    Digest, DigestAlgorithm, EntityKind, EvidenceType, LedgerSequence, Result, TransactionHash,
};
use amasario_provenance::{ArtifactType, RevisionKind};
use serde::{Deserialize, Serialize};

use crate::errors::{EvidenceFailure, first_failure};

/// The shortest a claim may be before it stops being one.
///
/// Eight characters, matching the schema's `minLength`. The field exists because a
/// record has to say what it supports, and a two-word label does not.
pub const MINIMUM_CLAIM_LENGTH: usize = 8;

/// The longest a note may be, matching the schema's `maxLength`.
///
/// Enforced rather than ignored: a reader validating the emitted document against the
/// schema would refuse one that exceeds it, so an engine that accepted it would produce
/// output it cannot itself read back.
pub const MAXIMUM_NOTE_LENGTH: usize = 2048;

/// An evidence class: a taxonomy term, or one this version does not recognise.
///
/// The unrecognised arm is not an error case to be routed around. It is the
/// specification's own instruction - an open vocabulary whose consumer "must handle
/// unknown" terms - made explicit in the type so that every place a class is used has to
/// decide what to do with one that is not understood.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EvidenceClass {
    /// A class this specification version defines.
    Recognised(EvidenceType),
    /// A term from a later version or another producer.
    Unrecognised(String),
}

impl EvidenceClass {
    /// The class for a term, recognised or not.
    #[must_use]
    pub fn parse(term: &str) -> Self {
        match term.parse::<EvidenceType>() {
            Ok(class) => Self::Recognised(class),
            Err(_) => Self::Unrecognised(term.to_owned()),
        }
    }

    /// The stable wire name.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Recognised(class) => class.as_str(),
            Self::Unrecognised(term) => term.as_str(),
        }
    }

    /// Whether this version understands the class.
    #[must_use]
    pub const fn is_recognised(&self) -> bool {
        matches!(self, Self::Recognised(_))
    }

    /// The recognised class, when this version understands it.
    #[must_use]
    pub const fn recognised(&self) -> Option<EvidenceType> {
        match self {
            Self::Recognised(class) => Some(*class),
            Self::Unrecognised(_) => None,
        }
    }

    /// What a reviewer consults to confirm evidence of this class.
    ///
    /// `None` for an unrecognised class, because the taxonomy's `traceableTo` is the term
    /// this version's definition supplies and there is no definition for a term it does
    /// not know. Returning a guess would be exactly the interpretation the open
    /// vocabulary forbids.
    #[must_use]
    pub const fn traceable_to(&self) -> Option<&'static str> {
        match self {
            Self::Recognised(EvidenceType::Source) => {
                Some("the source repository at the recorded revision")
            },
            Self::Recognised(EvidenceType::Build) => {
                Some("the build record and the inputs it declares")
            },
            Self::Recognised(EvidenceType::Artifact) => {
                Some("the artifact content addressed by the digest")
            },
            Self::Recognised(EvidenceType::Wasm) => {
                Some("the network observation at a recorded ledger boundary")
            },
            Self::Recognised(EvidenceType::Deployment) => {
                Some("the deployment transaction and the ledger it was included in")
            },
            Self::Recognised(EvidenceType::Transaction) => {
                Some("the transaction hash on a named network")
            },
            Self::Recognised(EvidenceType::Event) => {
                Some("the emitting transaction and the event index")
            },
            Self::Recognised(EvidenceType::Attestation) => {
                Some("the attestation issuer and the exact claim text")
            },
            Self::Recognised(EvidenceType::Observation) => {
                Some("the observation record with its timestamp and boundary")
            },
            Self::Recognised(_) => None,
            Self::Unrecognised(_) => None,
        }
    }

    /// Whether the class is derived from network observation and therefore needs a
    /// boundary.
    ///
    /// The taxonomy says a `WASM` record is "traceable to the network observation that
    /// reported it", and a `TRANSACTION`, `DEPLOYMENT` or `EVENT` record to a ledger and
    /// a transaction. All four derive from a chain, so all four need to say which chain
    /// and at what point in its history.
    #[must_use]
    pub const fn requires_boundary(&self) -> bool {
        matches!(
            self.recognised(),
            Some(
                EvidenceType::Wasm
                    | EvidenceType::Transaction
                    | EvidenceType::Deployment
                    | EvidenceType::Event
            )
        )
    }

    /// Whether the class addresses content and therefore needs a digest.
    #[must_use]
    pub const fn requires_digest(&self) -> bool {
        matches!(
            self.recognised(),
            Some(EvidenceType::Artifact | EvidenceType::Wasm)
        )
    }
}

impl std::fmt::Display for EvidenceClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One collected evidence record.
///
/// The fields mirror `schema/evidence.schema.json` exactly, including the per-class ones:
/// `repository` and `revision` for `SOURCE`, `toolchain` and `configuration_digest` for
/// `BUILD`, `digest` and `artifact_type` for `ARTIFACT`, `successful` for `TRANSACTION`,
/// `attestation_id` for `ATTESTATION`, `observation_note` for `OBSERVATION`. A record
/// carries all of them and [`Self::failures`] checks the ones its class requires, which is
/// what lets one type serve nine classes without nine near-identical types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    /// The record's identifier, unique within the collected set.
    pub id: String,
    /// The class the record belongs to.
    pub class: EvidenceClass,
    /// The exact claim the record supports, stated as a sentence.
    pub claim: String,
    /// Identifiers of the dependency edges or impact findings that rely on this record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supports: Vec<String>,
    /// Identifiers of records this one contradicts.
    ///
    /// Declared rather than inferred. The confidence schema records contradiction "in its
    /// own field rather than in the rationale" because that "is what makes the conflict
    /// machine-detectable", and the same reasoning applies one level down: two records
    /// that disagree are only detectably in conflict if one of them says so, since two
    /// digests that differ look exactly like two digests about different things.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradicts: Vec<String>,
    /// When this record was observed, as an RFC 3339 timestamp.
    pub observed_at: String,
    /// The network and ledger boundary of the observation, where the class derives from a
    /// chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<amasario_core::ObservationBoundary>,
    /// A digest over what the record points at, where the class is content-addressable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    /// `SOURCE` evidence: the repository the revision was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// `SOURCE` evidence: the revision observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// `SOURCE` evidence: what kind of revision it is.
    ///
    /// Carried so that the immutability rule can be applied without guessing: a branch
    /// name and a commit identifier are both strings, and only the kind distinguishes the
    /// one that denotes an immutable tree. The rule it feeds is
    /// [`crate::confidence::basis_for`]'s, not this record's: a mutable revision is valid
    /// evidence that a tree was read, and it is not decisive evidence of which tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_kind: Option<RevisionKind>,
    /// `BUILD` evidence: the toolchain identity the build used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    /// `BUILD` evidence: the digest of the build configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_digest: Option<Digest>,
    /// `ARTIFACT` evidence: the artifact class the digest was computed over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<ArtifactType>,
    /// `WASM`, `DEPLOYMENT` or `EVENT` evidence: the contract address concerned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    /// `TRANSACTION`, `DEPLOYMENT` or `EVENT` evidence: the transaction it derives from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionHash>,
    /// `TRANSACTION`, `DEPLOYMENT` or `EVENT` evidence: the ledger it derives from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger: Option<LedgerSequence>,
    /// `EVENT` evidence: the event's index within its emitting transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_index: Option<u32>,
    /// `TRANSACTION` evidence: whether the transaction succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successful: Option<bool>,
    /// `ATTESTATION` evidence: the identifier of the attestation this refers to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attestation_id: Option<String>,
    /// `ATTESTATION` evidence: the claim the attestation itself states.
    ///
    /// Required for the class so that the scope rule is checkable rather than trusted.
    /// "An attestation supports the specific claim it states and nothing broader" can
    /// only be enforced by comparing the cited claim with the attested one, and that
    /// comparison needs both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attested_claim: Option<String>,
    /// `OBSERVATION` evidence: what was observed, stated factually.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_note: Option<String>,
}

impl EvidenceRecord {
    /// Builds a record field by field, without validating it.
    ///
    /// Exists because a record's fields are filled in over several steps - a digest is
    /// computed, a boundary is attached, a transaction is looked up - and requiring each
    /// intermediate state to be valid would force a caller to invent placeholders. The
    /// validation happens where it matters, at [`Self::new`] and at
    /// [`EvidenceRegistry::add`], so a record that reaches a registry or a claim has been
    /// checked.
    #[must_use]
    pub fn draft(
        id: impl Into<String>,
        class: EvidenceClass,
        claim: impl Into<String>,
        observed_at: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            class,
            claim: claim.into(),
            supports: Vec::new(),
            contradicts: Vec::new(),
            observed_at: observed_at.into(),
            boundary: None,
            digest: None,
            repository: None,
            revision: None,
            revision_kind: None,
            toolchain: None,
            configuration_digest: None,
            artifact_type: None,
            contract_id: None,
            transaction: None,
            ledger: None,
            event_index: None,
            successful: None,
            attestation_id: None,
            attested_claim: None,
            observation_note: None,
        }
    }

    /// Builds a record, validating it against its class's requirements.
    ///
    /// # Errors
    ///
    /// Returns the first failure from [`Self::failures`].
    pub fn new(
        id: impl Into<String>,
        class: EvidenceClass,
        claim: impl Into<String>,
        observed_at: impl Into<String>,
    ) -> Result<Self> {
        let record = Self::draft(id, class, claim, observed_at);
        record.validate()?;
        Ok(record)
    }

    /// Records that this evidence supports a claim's or a relationship's identifier.
    #[must_use]
    pub fn supporting(mut self, target: impl Into<String>) -> Self {
        let target = target.into();
        if !self.supports.contains(&target) {
            self.supports.push(target);
        }
        self
    }

    /// Records that this evidence contradicts another record.
    #[must_use]
    pub fn contradicting(mut self, other: impl Into<String>) -> Self {
        let other = other.into();
        if !self.contradicts.contains(&other) {
            self.contradicts.push(other);
        }
        self
    }

    /// Attaches the observation boundary.
    #[must_use]
    pub fn with_boundary(mut self, boundary: amasario_core::ObservationBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Whether this record is permitted to support a claim.
    ///
    /// `false` for an unrecognised class, which is what treating such a record as
    /// `UNKNOWN` means: it is kept, and nothing is established by it.
    #[must_use]
    pub const fn supports_claim(&self) -> bool {
        self.class.is_recognised()
    }

    /// The reference a claim cites this record by.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the identifier is empty, which [`Self::new`] does
    /// not itself refuse because a record can be built field by field.
    pub fn reference(&self) -> Result<amasario_dependency::EvidenceRef> {
        let kind = match self.class.recognised() {
            Some(kind) => kind,
            // The reference type is a recognised kind, so an unrecognised class is
            // recorded as an observation: the engine can say that *something* was observed
            // and cannot claim what kind it was. The record's own class keeps the raw term,
            // so nothing is lost from the collected set.
            None => EvidenceType::Observation,
        };
        amasario_dependency::EvidenceRef::new(kind, self.id.clone())
    }

    /// Every way this record fails its class's requirements.
    ///
    /// Checks, in the order the taxonomy and schema state them:
    ///
    /// * the claim is stated and specific enough to be one;
    /// * an unrecognised class is accepted as-is, because the open vocabulary requires it
    ///   to be, and everything after this point applies only to recognised classes;
    /// * every class is traceable to what its `traceableTo` names - concretely, for
    ///   `SOURCE` a repository and a revision, for `BUILD` a toolchain and a configuration
    ///   digest, for `ARTIFACT` a digest and an artifact type, for the four chain-derived
    ///   classes a boundary;
    /// * a `TRANSACTION` record states its outcome, and a failed transaction is not cited
    ///   for a claim about its effect;
    /// * an `ATTESTATION` record names the attestation it refers to and states what that
    ///   attestation claims;
    /// * an `OBSERVATION` record says what was observed;
    /// * every note stays within the schema's bounds, so the document this engine emits is
    ///   one it can read back;
    /// * every digest is SHA-256, the only algorithm the specification defines.
    #[must_use]
    pub fn failures(&self) -> Vec<EvidenceFailure> {
        let mut failures: Vec<EvidenceFailure> = Vec::new();
        let class = self.class.as_str().to_owned();

        let claim_length = self.claim.chars().count();
        if claim_length < MINIMUM_CLAIM_LENGTH {
            failures.push(EvidenceFailure::ClaimNotStated {
                evidence: self.id.clone(),
                length: claim_length,
            });
        }

        // A digest under an algorithm the specification does not define cannot be compared
        // with one it does, so it is checked for every class a digest can appear on.
        for digest in [self.digest.as_ref(), self.configuration_digest.as_ref()]
            .into_iter()
            .flatten()
        {
            if digest.algorithm() != DigestAlgorithm::Sha256 {
                failures.push(EvidenceFailure::DigestAlgorithmUnsupported {
                    evidence: self.id.clone(),
                    algorithm: digest.algorithm().as_str().to_owned(),
                });
            }
        }

        let Some(kind) = self.class.recognised() else {
            // An unrecognised class is stored and inert. The record's own claim and
            // timestamp were already checked, and nothing further can be, because this
            // version does not know what the class requires.
            return failures;
        };

        if self.class.requires_boundary() && self.boundary.is_none() {
            failures.push(EvidenceFailure::BoundaryUnrecorded {
                evidence: self.id.clone(),
                class: class.clone(),
            });
        }

        match kind {
            EvidenceType::Source => {
                let traceable = self
                    .repository
                    .as_deref()
                    .is_some_and(|repository| !repository.is_empty())
                    && self
                        .revision
                        .as_deref()
                        .is_some_and(|revision| !revision.is_empty());
                if !traceable {
                    failures.push(EvidenceFailure::NotTraceable {
                        evidence: self.id.clone(),
                        class,
                        traceable_to: self
                            .class
                            .traceable_to()
                            .unwrap_or("the repository at the recorded revision")
                            .to_owned(),
                    });
                }
                // A mutable revision is deliberately *not* a failure here.
                //
                // `rules/provenance/source-to-build.yaml` requires that "a source revision
                // reached through a mutable ref MUST NOT be reported as VERIFIED". That is
                // a constraint on the verification status and the confidence, not on the
                // record's validity: a branch does pin a tree at the moment it is read, so
                // refusing the record would discard the only evidence that the source was
                // read at all. The consequence is carried where the rule puts it -
                // [`crate::confidence::basis_for`] rates a mutable revision
                // [`crate::confidence::EvidenceBasis::Directional`], which caps the
                // confidence at `MEDIUM_CONFIDENCE` and keeps `status_for` below
                // `VERIFIED`.
            },
            EvidenceType::Build => {
                if !self
                    .toolchain
                    .as_deref()
                    .is_some_and(|toolchain| !toolchain.is_empty())
                {
                    failures.push(EvidenceFailure::ToolchainUnrecorded {
                        evidence: self.id.clone(),
                    });
                }
                if self.configuration_digest.is_none() {
                    failures.push(EvidenceFailure::ConfigurationUnrecorded {
                        evidence: self.id.clone(),
                    });
                }
            },
            EvidenceType::Artifact | EvidenceType::Wasm => {
                if self.digest.is_none() {
                    failures.push(EvidenceFailure::DigestUnrecorded {
                        evidence: self.id.clone(),
                        class,
                    });
                }
                if kind == EvidenceType::Artifact && self.artifact_type.is_none() {
                    failures.push(EvidenceFailure::ArtifactTypeUnrecorded {
                        evidence: self.id.clone(),
                    });
                }
            },
            EvidenceType::Deployment => {
                if self.transaction.is_none() {
                    failures.push(EvidenceFailure::NotTraceable {
                        evidence: self.id.clone(),
                        class,
                        traceable_to: self
                            .class
                            .traceable_to()
                            .unwrap_or("the deployment transaction and ledger")
                            .to_owned(),
                    });
                }
                // A deployment that failed never took effect, so it cannot be cited as
                // evidence that an executable became a contract.
                if self.successful == Some(false) {
                    failures.push(EvidenceFailure::FailedTransactionCitedAsEffect {
                        transaction: self.transaction.as_ref().map_or_else(
                            || "an unrecorded transaction".to_owned(),
                            ToString::to_string,
                        ),
                        claim: self.claim.clone(),
                    });
                }
            },
            EvidenceType::Transaction => {
                if self.successful.is_none() {
                    failures.push(EvidenceFailure::TransactionOutcomeUnrecorded {
                        transaction: self
                            .transaction
                            .as_ref()
                            .map_or_else(|| self.id.clone(), ToString::to_string),
                    });
                }
                if self.transaction.is_none() {
                    failures.push(EvidenceFailure::NotTraceable {
                        evidence: self.id.clone(),
                        class,
                        traceable_to: self
                            .class
                            .traceable_to()
                            .unwrap_or("the transaction hash on a named network")
                            .to_owned(),
                    });
                }
            },
            EvidenceType::Event => {
                if self.transaction.is_none() || self.event_index.is_none() {
                    failures.push(EvidenceFailure::EventOriginUnrecorded {
                        evidence: self.id.clone(),
                    });
                }
            },
            EvidenceType::Attestation => {
                if !self
                    .attestation_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
                {
                    failures.push(EvidenceFailure::AttestationUnrecorded {
                        evidence: self.id.clone(),
                    });
                }
                if let Some(attested) = self.attested_claim.as_deref()
                    && !attested.is_empty()
                    && attested != self.claim
                {
                    failures.push(EvidenceFailure::AttestationClaimMismatch {
                        attestation: self.attestation_id.clone().unwrap_or_default(),
                        claim: self.claim.clone(),
                        attested: attested.to_owned(),
                    });
                }
            },
            EvidenceType::Observation => {
                if self.observation_note.is_none() {
                    failures.push(EvidenceFailure::ObservationNoteUnrecorded {
                        evidence: self.id.clone(),
                    });
                }
            },
            // `EvidenceType` is non-exhaustive, so a class added by a later revision
            // reaches this arm. It is stored and treated as this version treats an
            // unrecognised class: kept, and permitted to support nothing.
            _ => failures.push(EvidenceFailure::DigestAlgorithmUnsupported {
                evidence: self.id.clone(),
                algorithm: format!(
                    "unhandled evidence class {} at this engine version",
                    self.class.as_str()
                ),
            }),
        }

        // A note is a property of the record rather than of the class, so its length is
        // checked whenever one is present. `schema/evidence.schema.json` bounds it as
        // `minLength: 8`, `maxLength: 2048`, and a document that violated that bound would
        // be rejected by a reader even though this engine accepted it.
        if let Some(note) = self.observation_note.as_deref() {
            let length = note.chars().count();
            if length < MINIMUM_CLAIM_LENGTH {
                failures.push(EvidenceFailure::ObservationNoteUnrecorded {
                    evidence: self.id.clone(),
                });
            } else if length > MAXIMUM_NOTE_LENGTH {
                failures.push(EvidenceFailure::NoteTooLong {
                    evidence: self.id.clone(),
                    length,
                });
            }
        }

        failures
    }

    /// Validates the record, reporting the first requirement it fails.
    ///
    /// # Errors
    ///
    /// Returns the first failure from [`Self::failures`].
    pub fn validate(&self) -> Result<()> {
        first_failure(self.failures())
    }

    /// Whether the record satisfies every requirement of its class.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.failures().is_empty()
    }
}

/// The kind of revision a recorded revision of unknown type behaves like.
///
/// The inference is deliberately one-sided. A revision of 40 or 64 hexadecimal
/// characters is a content-addressed commit identifier - SHA-1 for git, SHA-256 for the
/// digests Stellar reports - and everything else is treated as a branch, which is the
/// mutable kind. The asymmetry is the point: treating a commit as a branch loses nothing
/// but confidence, while treating a branch as a commit would let a record that cannot be
/// checked be reported as verified, which is the one error the provenance model exists to
/// prevent. A tag is never inferred, because no tag name is distinguishable from a branch
/// name by its text.
#[must_use]
pub fn infer_revision_kind(revision: &str) -> RevisionKind {
    let hexadecimal = !revision.is_empty()
        && revision
            .chars()
            .all(|character| character.is_ascii_hexdigit());
    if hexadecimal && matches!(revision.len(), 40 | 64) {
        RevisionKind::Commit
    } else {
        RevisionKind::Branch
    }
}

/// The evidence an analysis collected, keyed by identifier.
///
/// Ordered canonically rather than by insertion, so two runs over the same inputs produce
/// the same document and a snapshot diff does not report a reordering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvidenceRegistry {
    records: Vec<EvidenceRecord>,
}

impl EvidenceRegistry {
    /// An empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Adds a record, refusing one that fails its class or repeats an identifier.
    ///
    /// # Errors
    ///
    /// Returns the record's first failure, or a duplicate-identifier failure when another
    /// record already holds its identifier.
    pub fn add(&mut self, record: EvidenceRecord) -> Result<()> {
        record.validate()?;
        if self.records.iter().any(|existing| existing.id == record.id) {
            return Err(EvidenceFailure::DuplicateEvidenceId { id: record.id }.into_error());
        }
        self.records.push(record);
        self.records.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.class.as_str().cmp(right.class.as_str()))
        });
        Ok(())
    }

    /// Adds every record, reporting the first that cannot be added.
    ///
    /// # Errors
    ///
    /// Returns the first failure from [`Self::add`].
    pub fn extend(&mut self, records: impl IntoIterator<Item = EvidenceRecord>) -> Result<()> {
        for record in records {
            self.add(record)?;
        }
        Ok(())
    }

    /// The record with an identifier.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&EvidenceRecord> {
        self.records.iter().find(|record| record.id == id)
    }

    /// Every record, in canonical order.
    #[must_use]
    pub fn records(&self) -> &[EvidenceRecord] {
        &self.records
    }

    /// How many records the registry holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the registry holds nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The records of one class.
    #[must_use]
    pub fn of_class(&self, class: EvidenceType) -> Vec<&EvidenceRecord> {
        self.records
            .iter()
            .filter(|record| record.class.recognised() == Some(class))
            .collect()
    }

    /// The records this version cannot interpret.
    ///
    /// Reported rather than hidden, because an analysis whose evidence includes terms
    /// from a version it does not know is one a consumer must not read as complete.
    #[must_use]
    pub fn unrecognised(&self) -> Vec<&EvidenceRecord> {
        self.records
            .iter()
            .filter(|record| !record.class.is_recognised())
            .collect()
    }

    /// The records that declare support for an identifier.
    #[must_use]
    pub fn supporting(&self, target: &str) -> Vec<&EvidenceRecord> {
        self.records
            .iter()
            .filter(|record| record.supports.iter().any(|id| id == target))
            .collect()
    }

    /// The references for a list of identifiers, in the order given.
    ///
    /// # Errors
    ///
    /// Returns a not-traceable failure naming the first identifier that is not present,
    /// because a citation to a record the set does not hold cannot be followed back to
    /// what it supports.
    pub fn references(&self, ids: &[String]) -> Result<Vec<amasario_dependency::EvidenceRef>> {
        let mut references = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(record) = self.get(id) else {
                return Err(EvidenceFailure::NotTraceable {
                    evidence: id.clone(),
                    class: "CITED".to_owned(),
                    traceable_to: "a record present in the collected evidence".to_owned(),
                }
                .into_error());
            };
            references.push(record.reference()?);
        }
        Ok(references)
    }

    /// The pairs of records that declare a contradiction with each other.
    ///
    /// Only declared contradictions are reported. Two records whose digests differ are not
    /// thereby in conflict - they may simply be about different artifacts - and inferring
    /// conflict from inequality would report every disagreement between unrelated records
    /// as a contradiction, which would make the `CONFLICTING` status meaningless.
    #[must_use]
    pub fn contradictions(&self) -> Vec<(&EvidenceRecord, &EvidenceRecord)> {
        let mut pairs: Vec<(&EvidenceRecord, &EvidenceRecord)> = Vec::new();
        for record in &self.records {
            for cited in &record.contradicts {
                if let Some(other) = self.get(cited) {
                    pairs.push((record, other));
                }
            }
        }
        pairs.sort_by(|left, right| {
            left.0
                .id
                .cmp(&right.0.id)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        pairs
    }

    /// Identifiers cited as contradicted by a record that is not present.
    ///
    /// A conflict names two records. One of them missing means the conflict cannot be
    /// evaluated, and reporting it as resolved would be worse than reporting it as
    /// unresolvable.
    #[must_use]
    pub fn unresolvable_contradictions(&self) -> Vec<EvidenceFailure> {
        let mut failures: Vec<EvidenceFailure> = Vec::new();
        for record in &self.records {
            for cited in &record.contradicts {
                if self.get(cited).is_none() {
                    failures.push(EvidenceFailure::ContradictionUnresolvable {
                        evidence: record.id.clone(),
                        cited: cited.clone(),
                    });
                }
            }
        }
        failures
    }

    /// Whether any evidence in the set is in declared conflict.
    #[must_use]
    pub fn has_conflict(&self) -> bool {
        !self.contradictions().is_empty()
    }

    /// The entity reference an evidence record concerns, where it names one.
    ///
    /// Used to attach evidence to the entity it is about rather than to the analysis, so
    /// that a report can show which record supports which claim.
    #[must_use]
    pub fn subject_of(record: &EvidenceRecord) -> Option<amasario_core::EntityRef> {
        if let Some(digest) = record.digest.as_ref()
            && record.class.requires_digest()
        {
            return amasario_core::EntityRef::new(
                if record.class.recognised() == Some(EvidenceType::Wasm) {
                    EntityKind::Wasm
                } else {
                    EntityKind::Artifact
                },
                digest.value(),
            )
            .ok();
        }
        record.contract_id.as_ref().and_then(|address| {
            amasario_core::EntityRef::new(EntityKind::Contract, address.clone()).ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::LedgerSequence;

    fn boundary() -> amasario_core::ObservationBoundary {
        amasario_core::ObservationBoundary::new(
            amasario_core::Network::new(
                "testnet",
                amasario_core::NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn digest(seed: u8) -> Digest {
        Digest::sha256_of(&[seed])
    }

    fn transaction() -> TransactionHash {
        TransactionHash::new("ab".repeat(32)).expect("a transaction hash")
    }

    /// A draft, which is how a record is assembled when its class is not yet satisfied.
    fn draft(class: &str, claim: &str) -> EvidenceRecord {
        EvidenceRecord::draft(
            "e-1",
            EvidenceClass::parse(class),
            claim,
            "2026-01-01T00:00:00Z",
        )
    }

    fn observation(note: &str) -> EvidenceRecord {
        let mut record = draft("OBSERVATION", "something was observed at the boundary");
        record.observation_note = Some(note.to_owned());
        record
    }

    #[test]
    fn an_unrecognised_class_is_stored_rather_than_rejected() {
        // `openness: open` with `consumersMustHandleUnknown: true`, and the schema's own
        // instruction to treat an unrecognised term as UNKNOWN.
        let class = EvidenceClass::parse("FUTURE_CLASS");
        assert!(!class.is_recognised());
        assert_eq!(class.as_str(), "FUTURE_CLASS");
        assert!(
            class.traceable_to().is_none(),
            "there is no definition to quote"
        );
        assert!(!class.requires_boundary());
        assert!(!class.requires_digest());

        let record = EvidenceRecord::new(
            "e-1",
            class,
            "a claim from a producer this version does not know",
            "2026-01-01T00:00:00Z",
        )
        .expect("an unrecognised class is still a valid record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert!(
            !record.supports_claim(),
            "it is kept and establishes nothing, which is what treating it as UNKNOWN means"
        );
        let mut registry = EvidenceRegistry::new();
        registry.add(record).expect("stored");
        assert_eq!(registry.unrecognised().len(), 1);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn every_recognised_class_quotes_what_a_reviewer_would_consult() {
        for class in EvidenceType::all() {
            let class = EvidenceClass::Recognised(*class);
            assert!(
                class.traceable_to().is_some(),
                "{class} declares no traceableTo"
            );
        }
        assert_eq!(
            EvidenceClass::parse("ARTIFACT").traceable_to(),
            Some("the artifact content addressed by the digest")
        );
    }

    #[test]
    fn a_claim_shorter_than_a_sentence_is_refused_for_every_class() {
        let error = EvidenceRecord::new(
            "e-1",
            EvidenceClass::parse("OBSERVATION"),
            "yes",
            "2026-01-01T00:00:00Z",
        )
        .expect_err("a three-character claim is not a claim");
        assert!(error.to_string().contains("3-character claim"));
    }

    #[test]
    fn a_transaction_record_must_state_its_outcome_and_its_hash() {
        let mut record = draft(
            "TRANSACTION",
            "the transaction carried a contract deployment",
        );
        let failures = record.failures();
        assert!(
            failures.iter().any(|failure| matches!(
                failure,
                EvidenceFailure::TransactionOutcomeUnrecorded { .. }
            )),
            "got: {failures:?}"
        );
        record.successful = Some(true);
        let failures = record.failures();
        assert!(
            failures
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::NotTraceable { .. })),
            "without a hash the record cannot be found again: {failures:?}"
        );
        record.transaction = Some(transaction());
        record.boundary = Some(boundary());
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn a_failed_deployment_cannot_be_cited_as_evidence_that_it_took_effect() {
        let mut record = draft(
            "DEPLOYMENT",
            "the executable became the contract at this address",
        );
        record.transaction = Some(transaction());
        record.boundary = Some(boundary());
        record.successful = Some(false);
        let failures = record.failures();
        assert!(
            failures.iter().any(|failure| matches!(
                failure,
                EvidenceFailure::FailedTransactionCitedAsEffect { .. }
            )),
            "got: {failures:?}"
        );
    }

    #[test]
    fn source_evidence_needs_a_repository_and_a_revision() {
        let mut record = draft(
            "SOURCE",
            "the artifact was built from this repository at this revision",
        );
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::NotTraceable { .. })),
            "a source record with neither repository nor revision is not traceable"
        );
        record.repository = Some("https://example.invalid/r".to_owned());
        record.revision = Some("main".to_owned());
        // A branch is a valid revision: it pins a tree at the moment it was read. What it
        // cannot support is a VERIFIED claim, which is the confidence layer's job rather
        // than this one's.
        assert!(
            record.is_valid(),
            "a branch names a tree as read: {:?}",
            record.failures()
        );
        assert_eq!(
            crate::confidence::basis_for(&record),
            crate::confidence::EvidenceBasis::Directional
        );

        record.revision = Some("a".repeat(40));
        assert!(
            record.is_valid(),
            "a 40-character hexadecimal identifier is a git commit: {:?}",
            record.failures()
        );
        assert_eq!(
            crate::confidence::basis_for(&record),
            crate::confidence::EvidenceBasis::Decisive
        );
    }

    #[test]
    fn an_observation_note_is_bounded_by_the_schema() {
        let mut record = draft("OBSERVATION", "the contract was seen at this boundary");
        record.observation_note = Some("short".to_owned());
        assert!(
            record.failures().iter().any(|failure| matches!(
                failure,
                EvidenceFailure::ObservationNoteUnrecorded { .. }
            )),
            "a four-character note states nothing: {:?}",
            record.failures()
        );

        // The minimum is inclusive, so exactly eight characters is a note.
        record.observation_note = Some("observed".to_owned());
        assert!(record.is_valid(), "{:?}", record.failures());

        record.observation_note = Some("x".repeat(MAXIMUM_NOTE_LENGTH + 1));
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::NoteTooLong { .. })),
            "a note longer than the schema permits must be refused: {:?}",
            record.failures()
        );
        record.observation_note = Some("x".repeat(MAXIMUM_NOTE_LENGTH));
        assert!(record.is_valid(), "the maximum is inclusive");
    }

    #[test]
    fn the_revision_kind_inference_is_one_sided() {
        assert_eq!(infer_revision_kind(&"a".repeat(40)), RevisionKind::Commit);
        assert_eq!(infer_revision_kind(&"a".repeat(64)), RevisionKind::Commit);
        assert_eq!(infer_revision_kind("main"), RevisionKind::Branch);
        assert_eq!(infer_revision_kind("v1.0.0"), RevisionKind::Branch);
        assert_eq!(infer_revision_kind(""), RevisionKind::Branch);
        // A 39-character string is not a commit identifier, and guessing that it might be
        // would be exactly the kind of guess that lets an uncheckable record look verified.
        assert_eq!(infer_revision_kind(&"a".repeat(39)), RevisionKind::Branch);
    }

    #[test]
    fn build_evidence_needs_a_toolchain_and_a_configuration_digest() {
        let mut record = draft("BUILD", "the artifact was produced by this recorded build");
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::ToolchainUnrecorded { .. }))
        );
        record.toolchain = Some("rustc 1.93.0, wasm32-unknown-unknown".to_owned());
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::ConfigurationUnrecorded { .. })),
            "got: {:?}",
            record.failures()
        );
        record.configuration_digest = Some(digest(2));
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn artifact_evidence_needs_a_digest_and_an_artifact_type() {
        let mut record = draft(
            "ARTIFACT",
            "the artifact content hashes to the recorded digest",
        );
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::DigestUnrecorded { .. }))
        );
        record.digest = Some(digest(1));
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::ArtifactTypeUnrecorded { .. })),
            "got: {:?}",
            record.failures()
        );
        record.artifact_type = Some(ArtifactType::Wasm);
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn the_chain_derived_classes_all_need_a_boundary() {
        for class in ["WASM", "TRANSACTION", "DEPLOYMENT", "EVENT"] {
            assert!(
                EvidenceClass::parse(class).requires_boundary(),
                "{class} derives from a chain and must say which one, and at what point"
            );
        }
        for class in ["SOURCE", "BUILD", "ARTIFACT", "ATTESTATION", "OBSERVATION"] {
            assert!(
                !EvidenceClass::parse(class).requires_boundary(),
                "{class} is not a network observation"
            );
        }

        let mut record = draft(
            "WASM",
            "the network reports this hash for the contract's executable",
        );
        record.digest = Some(digest(3));
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::BoundaryUnrecorded { .. })),
            "a ledger sequence is only meaningful against a chain"
        );
        record.boundary = Some(boundary());
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn an_event_record_needs_both_its_transaction_and_its_index() {
        let mut record = draft("EVENT", "the contract emitted an event naming its callee");
        record.boundary = Some(boundary());
        record.transaction = Some(transaction());
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::EventOriginUnrecorded { .. })),
            "an event without its index cannot be told from its siblings"
        );
        record.event_index = Some(0);
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn an_attestation_supports_only_the_claim_it_states() {
        let mut record = draft("ATTESTATION", "the contract is safe to depend on");
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::AttestationUnrecorded { .. })),
            "got: {:?}",
            record.failures()
        );
        record.attestation_id = Some("att-1".to_owned());
        record.attested_claim = Some("the artifact was built from revision abc1234".to_owned());
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| matches!(failure, EvidenceFailure::AttestationClaimMismatch { .. })),
            "got: {:?}",
            record.failures()
        );
        record.attested_claim = Some("the contract is safe to depend on".to_owned());
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn an_observation_must_state_what_was_observed() {
        let mut record = draft("OBSERVATION", "the observation boundary was reached");
        assert!(
            record.failures().iter().any(|failure| matches!(
                failure,
                EvidenceFailure::ObservationNoteUnrecorded { .. }
            )),
            "the note is the whole content of an observation"
        );
        record.observation_note = Some("the search stopped at ledger 1000".to_owned());
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn a_digest_is_sha256_because_that_is_the_only_algorithm_defined() {
        let mut record = draft("ARTIFACT", "the artifact content hashes to the digest");
        record.digest =
            Some(Digest::new(DigestAlgorithm::Sha512, &"a".repeat(128)).expect("a digest"));
        record.artifact_type = Some(ArtifactType::SourceArchive);
        let failures = record.failures();
        assert!(
            failures.iter().any(|failure| matches!(
                failure,
                EvidenceFailure::DigestAlgorithmUnsupported { .. }
            )),
            "got: {failures:?}"
        );
        record.digest = Some(digest(4));
        assert!(record.is_valid(), "{:?}", record.failures());
    }

    #[test]
    fn the_registry_refuses_a_duplicate_identifier() {
        let mut registry = EvidenceRegistry::new();
        let record = observation("the search stopped at the boundary");
        registry.add(record.clone()).expect("the first is stored");
        let error = registry
            .add(record)
            .expect_err("a duplicate identifier would make a citation ambiguous");
        assert!(error.to_string().contains("more than once"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_registry_orders_records_canonically_however_they_arrived() {
        let mut registry = EvidenceRegistry::new();
        let mut second = observation("the second record's observation");
        second.id = "e-2".to_owned();
        registry
            .extend(vec![second, observation("the first record's observation")])
            .expect("both stored");
        let ids: Vec<&str> = registry
            .records()
            .iter()
            .map(|record| record.id.as_str())
            .collect();
        assert_eq!(ids, vec!["e-1", "e-2"]);
    }

    #[test]
    fn only_declared_contradictions_are_conflicts() {
        let mut registry = EvidenceRegistry::new();
        let mut first = observation("the contract was observed at ledger 1000");
        first.id = "e-1".to_owned();
        let mut second = observation("the contract was observed at ledger 2000");
        second.id = "e-2".to_owned();
        registry.extend(vec![first, second]).expect("both stored");
        // Two observations that differ are not thereby in conflict: they may simply be
        // observations of different things, or of one thing at two times.
        assert!(!registry.has_conflict());
        assert!(registry.contradictions().is_empty());

        let mut third = observation("the contract was not observed at ledger 1000");
        third.id = "e-3".to_owned();
        third.contradicts.push("e-1".to_owned());
        registry.add(third).expect("stored");
        assert!(registry.has_conflict());
        assert_eq!(registry.contradictions().len(), 1);
        assert!(registry.unresolvable_contradictions().is_empty());
        assert_eq!(registry.contradictions()[0].0.id, "e-3");
        assert_eq!(registry.contradictions()[0].1.id, "e-1");
    }

    #[test]
    fn a_contradiction_with_a_missing_record_is_unresolvable_rather_than_resolved() {
        let mut registry = EvidenceRegistry::new();
        let mut record = observation("the contract was not observed at ledger 1000");
        record.contradicts.push("e-missing".to_owned());
        registry.add(record).expect("stored");
        let failures = registry.unresolvable_contradictions();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].to_string().contains("cannot be evaluated"));
        // A conflict whose other side is absent is not a conflict the registry can report,
        // and it is not silently dropped either.
        assert!(registry.contradictions().is_empty());
        assert!(!registry.has_conflict());
    }

    #[test]
    fn references_follow_the_records_they_name() {
        let mut registry = EvidenceRegistry::new();
        registry
            .add(observation("the search stopped at the boundary"))
            .expect("stored");
        let references = registry
            .references(&["e-1".to_owned()])
            .expect("the record is present");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].id, "e-1");
        assert_eq!(references[0].kind, EvidenceType::Observation);
        let error = registry
            .references(&["e-missing".to_owned()])
            .expect_err("a citation to an absent record cannot be followed");
        assert!(error.to_string().contains("e-missing"));
    }

    #[test]
    fn evidence_attaches_to_the_entity_it_concerns() {
        let mut record = draft(
            "WASM",
            "the network reports this hash for the contract's executable",
        );
        record.digest = Some(digest(3));
        record.boundary = Some(boundary());
        record.contract_id =
            Some("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_owned());
        let subject = EvidenceRegistry::subject_of(&record).expect("an executable subject");
        assert_eq!(subject.kind, EntityKind::Wasm);
        assert_eq!(subject.id, digest(3).value());

        // A class with a digest that is not content-addressed by kind falls back to the
        // contract address it names, so evidence is never attached to a guess.
        let mut attestation = draft(
            "ATTESTATION",
            "the artifact was built from revision abc1234",
        );
        attestation.attestation_id = Some("att-1".to_owned());
        attestation.attested_claim =
            Some("the artifact was built from revision abc1234".to_owned());
        assert!(EvidenceRegistry::subject_of(&attestation).is_none());
    }
}
