//! The evidence document the specification defines.
//!
//! # Why a projection, again
//!
//! [`crate::collector::EvidenceRecord`] is the engine's collected record, and the
//! module documentation has always claimed it "mirrors `schema/evidence.schema.json`
//! field for field". It does not, and the claim is the reason this module exists: a
//! reader trusting that sentence would have expected the record to marshal directly
//! into a schema-valid document, and it cannot.
//!
//! | Internal | `evidence.schema.json` |
//! | --- | --- |
//! | `class` | `type` |
//! | `supports` | `supportsRelationships` |
//! | `transaction` | `transactionHash` |
//! | `contradicts`, `revisionKind`, `eventIndex`, `attestedClaim` | no field at all |
//!
//! The schema sets `additionalProperties: false`, so the four fields with no home make
//! every document invalid while they are present. They are dropped here rather than
//! renamed into something they are not.
//!
//! # What dropping them costs, and why it is acceptable
//!
//! **`contradicts`.** Nothing is lost in substance: contradiction travels in
//! `confidence.schema.json`, which has a `contradictingEvidence` array precisely so
//! that conflict is machine-detectable. The record-level field is the engine's own
//! link between two records; the schema states the conflict where a consumer looks
//! for it.
//!
//! **`revisionKind`.** The schema distinguishes an immutable revision from a mutable
//! one through the `revision` value and the verification status, and
//! `taxonomies/verification-statuses.yaml` is where that judgement belongs.
//!
//! **`eventIndex`.** Genuinely not representable. The schema has no field for an
//! event's position within its transaction, so a consumer cannot tell two events of
//! one transaction apart by index alone. This is a narrowing, not a rename, and it is
//! recorded here rather than hidden: an event cited in a published document is
//! identified by its `id`.
//!
//! **`attestedClaim`.** Also genuinely not representable. The record-level rule the
//! engine enforces - "an attestation supports the specific claim it states and
//! nothing broader" - is checkable in the engine because both claims are present. A
//! published document carries the attestation's own `claim`, and
//! `attestation.schema.json` is where the scope limitations live.
//!
//! Each of these is a candidate for a specification proposal rather than something the
//! engine should work around. Dropping them keeps the engine honest: the document says
//! exactly what the schema allows and no more.

use amasario_core::{ObservationBoundary, Result};
use serde::{Deserialize, Serialize};

use crate::collector::{EvidenceClass, EvidenceRecord};

/// One evidence record, in the shape `evidence.schema.json` defines.
///
/// Required by the schema: `id`, `type`, `claim` and `observedAt`. Every other field is
/// per-class and absent when the class does not use it, and an absent field is omitted
/// rather than written as `null` so that "not recorded" and "recorded as empty" cannot
/// be confused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceDocument {
    /// The record's identifier, unique within the collected set.
    pub id: String,
    /// The evidence class.
    ///
    /// The schema's field is named `type`; `class` is the Rust name and the explicit
    /// rename wins over `rename_all`.
    #[serde(rename = "type")]
    pub class: EvidenceClass,
    /// The exact claim the record supports, stated as a sentence.
    pub claim: String,
    /// The dependency edges or impact findings this evidence supports.
    ///
    /// The schema requires at least one entry when the field is present, so an empty
    /// list is omitted rather than published as `[]`.
    ///
    /// `default` accompanies the omission so that a document the engine wrote is a
    /// document the engine can read; a field that is written only when non-empty must be
    /// read as absent when it is missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supports_relationships: Vec<String>,
    /// When this record was observed, as an RFC 3339 timestamp.
    pub observed_at: String,
    /// The network and ledger boundary of the observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// A digest over what the record points at, where it is content-addressable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<amasario_core::Digest>,
    /// `SOURCE` evidence: the repository the revision was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// `SOURCE` evidence: the revision observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// `BUILD` evidence: the toolchain identity the build used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    /// `BUILD` evidence: the digest of the build configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_digest: Option<amasario_core::Digest>,
    /// `ARTIFACT` evidence: the artifact class the digest was computed over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<amasario_provenance::ArtifactType>,
    /// `WASM`, `DEPLOYMENT` or `EVENT` evidence: the contract address concerned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    /// `TRANSACTION`, `DEPLOYMENT` or `EVENT` evidence: the transaction it derives from.
    ///
    /// Published as `transactionHash`, which is the schema's name for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_hash: Option<amasario_core::TransactionHash>,
    /// `TRANSACTION`, `DEPLOYMENT` or `EVENT` evidence: the ledger it derives from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger: Option<amasario_core::LedgerSequence>,
    /// `TRANSACTION` evidence: whether the transaction succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successful: Option<bool>,
    /// `ATTESTATION` evidence: the identifier of the attestation this refers to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attestation_id: Option<String>,
    /// `OBSERVATION` evidence: what was observed, stated factually.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_note: Option<String>,
}

impl EvidenceDocument {
    /// Projects a collected record into the document the specification defines.
    ///
    /// The four fields with no schema home - `contradicts`, `revisionKind`,
    /// `eventIndex` and `attestedClaim` - are deliberately not carried across; see the
    /// module documentation for what that costs and why it is the honest projection.
    #[must_use]
    pub fn of(record: &EvidenceRecord) -> Self {
        Self {
            id: record.id.clone(),
            class: record.class.clone(),
            claim: record.claim.clone(),
            supports_relationships: record.supports.clone(),
            observed_at: record.observed_at.clone(),
            boundary: record.boundary.clone(),
            digest: record.digest.clone(),
            repository: record.repository.clone(),
            revision: record.revision.clone(),
            toolchain: record.toolchain.clone(),
            configuration_digest: record.configuration_digest.clone(),
            artifact_type: record.artifact_type,
            contract_id: record.contract_id.clone(),
            transaction_hash: record.transaction.clone(),
            ledger: record.ledger,
            successful: record.successful,
            attestation_id: record.attestation_id.clone(),
            observation_note: record.observation_note.clone(),
        }
    }

    /// Projects a whole registry into documents, in the registry's own deterministic
    /// order.
    #[must_use]
    pub fn all(records: &[EvidenceRecord]) -> Vec<Self> {
        records.iter().map(Self::of).collect()
    }

    /// The document's JSON.
    ///
    /// # Errors
    ///
    /// Returns an evidence error if the document cannot be serialised, which for this
    /// shape would be a defect in the engine.
    pub fn to_value(&self) -> Result<serde_json::Value> {
        serde_json::to_value(self).map_err(|error| amasario_core::EngineError::Validation {
            path: "/evidence".to_owned(),
            detail: format!("the evidence document could not be written: {error}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::EvidenceRecord;
    use amasario_core::{EvidenceType, LedgerSequence, Network, NetworkType};

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(7_777).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn source_record() -> EvidenceRecord {
        let mut record = EvidenceRecord::draft(
            "ev-source-1",
            EvidenceClass::Recognised(EvidenceType::Source),
            "the source tree at revision 9f1c was read from the stated repository",
            "2026-09-15T00:00:00Z",
        );
        record.repository = Some("https://github.com/example/contract".to_owned());
        record.revision = Some("9f1c4b2e5d6a7b8c9d0e1f2a3b4c5d6e7f8091a2".to_owned());
        record.revision_kind = Some(amasario_provenance::RevisionKind::Commit);
        let failures = record.failures();
        assert!(failures.is_empty(), "the record is invalid: {failures:?}");
        record
    }

    fn transaction_record() -> EvidenceRecord {
        let mut record = EvidenceRecord::draft(
            "ev-tx-1",
            EvidenceClass::Recognised(EvidenceType::Transaction),
            "the deployment transaction was included and succeeded",
            "2026-09-15T00:00:00Z",
        );
        record.boundary = Some(boundary());
        record.transaction =
            Some(amasario_core::TransactionHash::new("a".repeat(64)).expect("a transaction hash"));
        record.ledger = Some(LedgerSequence::new(7_777).expect("a ledger"));
        record.successful = Some(true);
        record
    }

    #[test]
    fn a_document_carries_only_the_schemas_fields() {
        let document = EvidenceDocument::of(&source_record());
        let value = document.to_value().expect("serialises");
        let object = value.as_object().expect("an object");

        // `evidence.schema.json` sets `additionalProperties: false`, so any field the
        // engine invented would make the document invalid.
        let allowed = [
            "id",
            "type",
            "claim",
            "supportsRelationships",
            "observedAt",
            "boundary",
            "digest",
            "repository",
            "revision",
            "toolchain",
            "configurationDigest",
            "artifactType",
            "contractId",
            "transactionHash",
            "ledger",
            "successful",
            "attestationId",
            "observationNote",
        ];
        for key in object.keys() {
            assert!(
                allowed.contains(&key.as_str()),
                "{key} is not an evidence.schema.json field"
            );
        }
        for required in ["id", "type", "claim", "observedAt"] {
            assert!(object.contains_key(required), "missing required {required}");
        }
    }

    #[test]
    fn the_internal_names_do_not_leak_onto_the_wire() {
        // Each of these was the engine's own name and would be an additional property.
        let value = EvidenceDocument::of(&source_record())
            .to_value()
            .expect("serialises");
        for forbidden in [
            "class",
            "supports",
            "transaction",
            "contradicts",
            "revisionKind",
            "eventIndex",
            "attestedClaim",
            "observed_at",
            "configuration_digest",
            "observation_note",
        ] {
            assert!(
                value.get(forbidden).is_none(),
                "{forbidden} is an internal name, not a schema field"
            );
        }
        assert_eq!(value["type"], "SOURCE");
    }

    #[test]
    fn a_transaction_hash_is_published_under_the_schemas_name() {
        let value = EvidenceDocument::of(&transaction_record())
            .to_value()
            .expect("serialises");
        assert_eq!(value["transactionHash"], "a".repeat(64));
        assert_eq!(value["successful"], true);
        assert_eq!(value["ledger"], 7_777);
        assert!(value.get("transaction").is_none());
    }

    #[test]
    fn an_absent_per_class_field_is_omitted_rather_than_written_as_null() {
        // The schema's optional fields carry per-class meaning, and a `null` would say
        // "recorded as nothing" where the truth is "not applicable to this class".
        let value = EvidenceDocument::of(&source_record())
            .to_value()
            .expect("serialises");
        for absent in [
            "toolchain",
            "configurationDigest",
            "artifactType",
            "contractId",
            "transactionHash",
            "successful",
            "attestationId",
            "observationNote",
            "boundary",
            "digest",
        ] {
            assert!(
                value.get(absent).is_none(),
                "{absent} should be absent, not null"
            );
        }
    }

    #[test]
    fn an_empty_supports_list_is_omitted_because_the_schema_forbids_an_empty_one() {
        // `supportsRelationships` has `minItems: 1`, so publishing `[]` would be
        // invalid; omitting it is correct and says the same thing.
        let mut record = source_record();
        assert!(
            EvidenceDocument::of(&record)
                .to_value()
                .expect("serialises")
                .get("supportsRelationships")
                .is_none()
        );

        record.supports = vec!["edge-1".to_owned()];
        let value = EvidenceDocument::of(&record)
            .to_value()
            .expect("serialises");
        assert_eq!(value["supportsRelationships"][0], "edge-1");
    }

    #[test]
    fn the_field_with_no_schema_home_is_dropped_rather_than_renamed() {
        // `revisionKind` has no `evidence.schema.json` field. Renaming it into
        // something the schema does define would be inventing a meaning; dropping it
        // is a narrowing, and the module documentation says so.
        let record = source_record();
        assert!(record.revision_kind.is_some(), "the record does carry one");
        let value = EvidenceDocument::of(&record)
            .to_value()
            .expect("serialises");
        assert!(
            value.get("revisionKind").is_none(),
            "a field the schema forbids must not be published under any name"
        );
        assert_eq!(
            value["revision"],
            record.revision.as_deref().expect("a revision")
        );
    }

    #[test]
    fn every_record_class_projects_without_inventing_a_field() {
        use amasario_provenance::ArtifactType;
        let cases = [
            EvidenceType::Source,
            EvidenceType::Build,
            EvidenceType::Artifact,
            EvidenceType::Wasm,
            EvidenceType::Deployment,
            EvidenceType::Transaction,
            EvidenceType::Event,
            EvidenceType::Attestation,
            EvidenceType::Observation,
        ];
        for class in cases {
            let mut record = EvidenceRecord::draft(
                format!("ev-{}", class.as_str().to_lowercase()),
                EvidenceClass::Recognised(class),
                format!(
                    "a claim of at least eight characters for {}",
                    class.as_str()
                ),
                "2026-09-15T00:00:00Z",
            );
            record.boundary = Some(boundary());
            record.artifact_type = Some(ArtifactType::Wasm);
            let document = EvidenceDocument::of(&record);
            let value = document.to_value().expect("serialises");
            let object = value.as_object().expect("an object");
            assert_eq!(value["type"], class.as_str());
            assert!(
                object.len() >= 4,
                "the required fields are present for {}",
                class.as_str()
            );
            assert!(
                object.contains_key("artifactType"),
                "a set per-class field is published under its schema name"
            );
        }
    }

    #[test]
    fn an_unrecognised_class_is_published_as_the_term_it_arrived_as() {
        // `taxonomies/evidence-types.yaml` is open with `consumersMustHandleUnknown`,
        // so the engine keeps the raw term rather than guessing a class. The document
        // states what arrived; whether a consumer accepts it is the consumer's rule.
        let mut record = EvidenceRecord::draft(
            "ev-future",
            EvidenceClass::Unrecognised("FUTURE_CLASS".to_owned()),
            "a claim from a producer of a later specification version",
            "2026-09-15T00:00:00Z",
        );
        record.boundary = Some(boundary());
        let value = EvidenceDocument::of(&record)
            .to_value()
            .expect("serialises");
        assert_eq!(value["type"], "FUTURE_CLASS");
    }

    #[test]
    fn the_projection_is_deterministic() {
        let record = transaction_record();
        let first = EvidenceDocument::of(&record)
            .to_value()
            .expect("serialises");
        let second = EvidenceDocument::of(&record)
            .to_value()
            .expect("serialises");
        assert_eq!(first, second);
        assert_eq!(EvidenceDocument::all(&[record.clone(), record]).len(), 2);
    }
}
