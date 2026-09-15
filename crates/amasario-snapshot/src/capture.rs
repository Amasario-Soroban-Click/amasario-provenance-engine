//! The snapshot, and the builder that assembles one from an analysis.
//!
//! # What a snapshot is for
//!
//! A snapshot is the only artefact of an analysis that outlives the run. Everything else
//! the engine produces cites a network that will have moved on by the time anyone reads
//! it; a snapshot records what was seen, at which boundary, on which chain, with which
//! evidence, so that a later comparison compares two recorded states rather than two
//! recollections. `docs/snapshots.md` in the specification states what that requires of the
//! model: it must be able to contain the contract identity, the executable identity, the
//! provenance record, the dependency graph, the evidence, the confidence, the impact
//! information, the observation boundary, the network, the ledger boundary, the
//! specification version and the engine version.
//!
//! Each of those is a field below, and the ones the schema requires are not optional here.
//! `evidence` and `confidence` are required because a snapshot with no evidence "can
//! support no claim"; `boundary`, `network` and `ledgerBoundary` are required because a
//! snapshot that does not say where and when it looked cannot be compared with anything.
//!
//! # Why the identifier is derived rather than chosen
//!
//! `schema/snapshot.schema.json` requires the identifier to be "stable given the same
//! contract and boundary, so that re-capturing the same state yields the same identifier
//! rather than a new one". A chosen identifier would make every re-capture a new document,
//! and a diff between two captures of one unchanged state would be a diff between two
//! names for the same thing. [`snapshot_id`] derives it from the chain, the contract
//! address and the ledger, and [`Snapshot::failures`] recomputes it and refuses a document
//! whose identifier is not the one its inputs produce - which is what makes an edited
//! identifier detectable rather than merely discouraged.
//!
//! # Why the boundary is echoed at the top level
//!
//! The schema surfaces `network` and `ledgerBoundary` "so that a snapshot is
//! self-describing without walking into the boundary", and
//! `rules/provenance/contract-to-deployment` requires them to equal `boundary.network` and
//! `boundary.ledger`. The duplication is deliberate and the equality is enforced: a
//! snapshot whose self-description disagreed with its own boundary would leave a reader
//! choosing which of two answers to believe about the one thing that decides whether the
//! analysis is even about their chain.

use amasario_contract::ContractIdentity;
use amasario_core::{
    Confidence, ConfidenceLevel, Digest, EngineError, LedgerSequence, Network, ObservationBoundary,
    Result, SUPPORTED_API_VERSION, SUPPORTED_SPEC_VERSION,
};
use amasario_dependency::DependencySet;
use amasario_evidence::collector::EvidenceRecord;
use amasario_graph::GraphDocument;
use amasario_impact::ImpactFinding;
use amasario_provenance::{ArtifactIdentity, Attestation, ProvenanceChain};
use serde::{Deserialize, Serialize};

use crate::errors::SnapshotFailure;
use crate::normalize::{
    DEFAULT_VOLATILE_FIELDS, canonical_order_failures, canonicalise, content_digest,
    validate_volatile_fields, verify_content_digest,
};

/// The prefix every snapshot identifier carries.
///
/// Present so that an identifier is recognisable as one in a directory listing or a log
/// line, which is where a reader most often meets it.
pub const SNAPSHOT_ID_PREFIX: &str = "snapshot-";

/// How many hex characters of the derivation digest the identifier keeps.
///
/// Fixed rather than inherited from the digest, so that changing the digest algorithm
/// would not silently change the length of every identifier a consumer may already hold.
/// 128 bits of a content-derived digest is far more than enough to distinguish contracts
/// observed by one analysis corpus.
const DERIVED_ID_CHARACTERS: usize = 32;

/// The identifier a contract and a boundary derive.
///
/// Deterministic, and independent of when the capture ran - which is the whole
/// requirement: the same contract observed at the same ledger on the same chain is the
/// same capture whether it was taken now or last week. The parts are joined with a
/// character that cannot occur in a network name, a passphrase, a contract address or a
/// ledger number, so two different inputs cannot produce one identifier by concatenation.
#[must_use]
pub fn snapshot_id(contract: &ContractIdentity, boundary: &ObservationBoundary) -> String {
    let material = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        boundary.network.id,
        boundary.network.passphrase,
        contract.contract_id.as_str(),
        boundary.ledger.get()
    );
    let digest = Digest::sha256_of(material.as_bytes());
    let value = digest.value();
    let short = &value[..DERIVED_ID_CHARACTERS.min(value.len())];
    format!("{SNAPSHOT_ID_PREFIX}{short}")
}

/// One recorded analysis, at one boundary, on one chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// The specification's `apiVersion` for the family this document belongs to.
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    /// The normative specification version that produced the document.
    #[serde(rename = "specVersion")]
    pub spec_version: String,
    /// The snapshot's identifier, derived from the contract and the boundary.
    pub id: String,
    /// When the capture ran, as an RFC 3339 timestamp. Excluded from the content digest.
    #[serde(rename = "capturedAt")]
    pub captured_at: String,
    /// The observation boundary: the chain and the ledger everything here was read at.
    pub boundary: ObservationBoundary,
    /// The network, surfaced at the top level so the snapshot is self-describing.
    pub network: Network,
    /// The ledger boundary, surfaced for the same reason.
    #[serde(rename = "ledgerBoundary")]
    pub ledger_boundary: LedgerSequence,
    /// The contract identity this snapshot is about.
    pub contract: ContractIdentity,
    /// The deployed executable's identity, when it was established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm: Option<ArtifactIdentity>,
    /// The provenance record, when it was established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProvenanceChain>,
    /// The dependency set, including its truncation disclosure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<DependencySet>,
    /// The dependency graph, when the analysis materialised one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphDocument>,
    /// Every evidence record in this snapshot. Required and non-empty.
    pub evidence: Vec<EvidenceRecord>,
    /// The overall confidence in this snapshot's contents.
    pub confidence: Confidence,
    /// Impact findings computed at this boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub impact: Vec<ImpactFinding>,
    /// Attestations this snapshot cites.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestations: Vec<Attestation>,
    /// The engine version that produced this snapshot.
    #[serde(rename = "engineVersion", skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    /// The digest of the canonical form with the volatile fields excluded.
    #[serde(rename = "contentDigest")]
    pub content_digest: Digest,
    /// The field paths excluded from `contentDigest`.
    #[serde(rename = "volatileFields")]
    pub volatile_fields: Vec<String>,
    /// Whether any part of the snapshot was bounded before exhaustion.
    pub truncated: bool,
}

impl Snapshot {
    /// Every way this snapshot disagrees with what the schema requires of it.
    ///
    /// Returned as a list rather than stopping at the first, because a producer fixing a
    /// document wants to see all of its problems at once - the opposite of the
    /// finding-level checks in the other layers, where the first failure is the actionable
    /// one because a finding is rebuilt rather than edited.
    #[must_use]
    pub fn failures(&self) -> Vec<SnapshotFailure> {
        let mut failures: Vec<SnapshotFailure> = Vec::new();

        if self.id.is_empty() {
            failures.push(SnapshotFailure::SnapshotIdMissing);
        } else {
            let derived = snapshot_id(&self.contract, &self.boundary);
            if derived != self.id {
                failures.push(SnapshotFailure::SnapshotIdNotStable {
                    recorded: self.id.clone(),
                    derived,
                });
            }
        }

        // The boundary echo, compared field by field rather than as whole structs so that
        // the refusal can name which of the two disagreed and print both values.
        if self.network.id != self.boundary.network.id {
            failures.push(SnapshotFailure::BoundaryEchoDisagrees {
                field: "/network".to_owned(),
                scattered: self.network.id.clone(),
                boundary: self.boundary.network.id.clone(),
            });
        }
        if self.network.passphrase != self.boundary.network.passphrase {
            failures.push(SnapshotFailure::BoundaryEchoDisagrees {
                field: "/network/passphrase".to_owned(),
                scattered: self.network.passphrase.clone(),
                boundary: self.boundary.network.passphrase.clone(),
            });
        }
        if self.ledger_boundary != self.boundary.ledger {
            failures.push(SnapshotFailure::BoundaryEchoDisagrees {
                field: "/ledgerBoundary".to_owned(),
                scattered: self.ledger_boundary.get().to_string(),
                boundary: self.boundary.ledger.get().to_string(),
            });
        }

        if self.evidence.is_empty() {
            failures.push(SnapshotFailure::NoEvidenceRecorded);
        }

        failures.extend(validate_volatile_fields(&self.volatile_fields));
        failures.extend(canonical_order_failures(self));

        if self.spec_version != SUPPORTED_SPEC_VERSION {
            failures.push(SnapshotFailure::SpecVersionIncompatible {
                found: self.spec_version.clone(),
                supported: SUPPORTED_SPEC_VERSION.to_owned(),
            });
        }

        // The digest is checked last and reported as a single failure: a digest computed
        // over content that also violates something else would disagree for that reason,
        // and reporting both would bury the cause under the symptom.
        if failures.is_empty()
            && let Err(error) = verify_content_digest(self)
        {
            failures.push(SnapshotFailure::ContentDigestDisagrees {
                recorded: self.content_digest.prefixed(),
                computed: error.to_string(),
            });
        }

        failures
    }

    /// Whether the snapshot satisfies every requirement.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.failures().is_empty()
    }

    /// Validates the snapshot, reporting the first failure.
    ///
    /// # Errors
    ///
    /// Returns the first failure from [`Self::failures`].
    pub fn validate(&self) -> Result<()> {
        crate::errors::first_failure(self.failures())
    }

    /// Every evidence identifier this snapshot carries, in canonical order.
    ///
    /// A reader asking which record backs a dependency edge or an impact finding follows
    /// the same route regardless of which section the claim lives in: every citation in
    /// this document names one of these identifiers.
    #[must_use]
    pub fn evidence_ids(&self) -> Vec<String> {
        self.evidence
            .iter()
            .map(|record| record.id.clone())
            .collect()
    }

    /// The confidence level of the weakest part of this snapshot.
    ///
    /// The vocabulary's aggregation rule is the minimum ordinal - "a chain is only as
    /// strong as its weakest link" - applied here across the snapshot's own confidence and
    /// every impact finding's, so that a snapshot cannot present a stronger overall
    /// confidence than one of the findings it carries.
    #[must_use]
    pub fn weakest_level(&self) -> ConfidenceLevel {
        ConfidenceLevel::weakest_of(
            std::iter::once(self.confidence.level)
                .chain(self.impact.iter().map(|finding| finding.confidence.level)),
        )
    }
}

/// Assembles a snapshot from the parts an analysis produced.
///
/// The builder exists so that normalisation and the digest happen in one place and in the
/// only order that works: collect the parts, normalise them, derive the identifier, compute
/// the digest over the normalised form, then validate. A caller that set `content_digest`
/// itself would be computing a digest over content it had not finished writing.
#[derive(Debug, Clone)]
pub struct Capture {
    contract: ContractIdentity,
    boundary: ObservationBoundary,
    wasm: Option<ArtifactIdentity>,
    provenance: Option<ProvenanceChain>,
    dependencies: Option<DependencySet>,
    graph: Option<GraphDocument>,
    evidence: Vec<EvidenceRecord>,
    impact: Vec<ImpactFinding>,
    attestations: Vec<Attestation>,
    engine_version: Option<String>,
    truncated: bool,
}

impl Capture {
    /// Starts a capture of one contract at one boundary.
    #[must_use]
    pub const fn of(contract: ContractIdentity, boundary: ObservationBoundary) -> Self {
        Self {
            contract,
            boundary,
            wasm: None,
            provenance: None,
            dependencies: None,
            graph: None,
            evidence: Vec::new(),
            impact: Vec::new(),
            attestations: Vec::new(),
            engine_version: None,
            truncated: false,
        }
    }

    /// Records the deployed executable's identity.
    #[must_use]
    pub fn with_wasm(mut self, wasm: ArtifactIdentity) -> Self {
        self.wasm = Some(wasm);
        self
    }

    /// Records the provenance chain.
    #[must_use]
    pub fn with_provenance(mut self, provenance: ProvenanceChain) -> Self {
        self.provenance = Some(provenance);
        self
    }

    /// Records the dependency set.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: DependencySet) -> Self {
        self.dependencies = Some(dependencies);
        self
    }

    /// Records the dependency graph.
    #[must_use]
    pub fn with_graph(mut self, graph: GraphDocument) -> Self {
        self.graph = Some(graph);
        self
    }

    /// Records impact findings.
    #[must_use]
    pub fn with_impact(mut self, impact: Vec<ImpactFinding>) -> Self {
        self.impact = impact;
        self
    }

    /// Records the attestations this snapshot cites.
    #[must_use]
    pub fn with_attestations(mut self, attestations: Vec<Attestation>) -> Self {
        self.attestations = attestations;
        self
    }

    /// Records which engine produced the analysis.
    #[must_use]
    pub fn with_engine_version(mut self, version: impl Into<String>) -> Self {
        self.engine_version = Some(version.into());
        self
    }

    /// Records whether the analysis was bounded before exhaustion.
    ///
    /// The schema requires that "when true, the snapshot does not claim completeness", so
    /// this is taken from what the analysis reported rather than decided here. An analysis
    /// that truncated anything - a traversal that reached its depth bound, an event window
    /// that was closed - must say so, and a snapshot that omitted the flag would present a
    /// bounded result as an exhaustive one.
    #[must_use]
    pub const fn truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// Adds one evidence record.
    ///
    /// # Errors
    ///
    /// Returns a snapshot error when the record shares an identifier with one already
    /// collected, because claims cite evidence by identifier and a duplicate would make
    /// every citation of it ambiguous between the two.
    pub fn add_evidence(&mut self, record: EvidenceRecord) -> Result<()> {
        if self
            .evidence
            .iter()
            .any(|existing| existing.id == record.id)
        {
            return Err(EngineError::Snapshot(format!(
                "two evidence records share the identifier {:?}; a claim citing it could not say \
                 which record it meant",
                record.id
            )));
        }
        self.evidence.push(record);
        Ok(())
    }

    /// Builds the snapshot, normalising it and computing its digest.
    ///
    /// # Errors
    ///
    /// Returns a snapshot error when no evidence was collected - the schema requires a
    /// non-empty array, and the engine will not invent a record to satisfy it - or when the
    /// assembled snapshot fails any other requirement.
    pub fn build(self, captured_at: impl Into<String>) -> Result<Snapshot> {
        if self.evidence.is_empty() {
            return Err(SnapshotFailure::NoEvidenceRecorded.into_error());
        }

        let confidence = overall_confidence(&self.evidence, &self.impact);
        let mut snapshot = Snapshot {
            api_version: SUPPORTED_API_VERSION.to_owned(),
            spec_version: SUPPORTED_SPEC_VERSION.to_owned(),
            id: String::new(),
            captured_at: captured_at.into(),
            network: self.boundary.network.clone(),
            ledger_boundary: self.boundary.ledger,
            boundary: self.boundary,
            contract: self.contract,
            wasm: self.wasm,
            provenance: self.provenance,
            dependencies: self.dependencies,
            graph: self.graph,
            evidence: self.evidence,
            confidence,
            impact: self.impact,
            attestations: self.attestations,
            engine_version: self.engine_version,
            // Replaced below. The digest is excluded from the digested form, so the value
            // here never reaches the digest and cannot be mistaken for a computed one.
            content_digest: Digest::sha256_of(b""),
            volatile_fields: DEFAULT_VOLATILE_FIELDS
                .iter()
                .map(ToString::to_string)
                .collect(),
            truncated: self.truncated,
        };

        canonicalise(&mut snapshot);
        snapshot.id = snapshot_id(&snapshot.contract, &snapshot.boundary);
        snapshot.confidence = overall_confidence(&snapshot.evidence, &snapshot.impact);
        snapshot.content_digest = content_digest(&snapshot)?;
        snapshot.validate()?;
        Ok(snapshot)
    }
}

/// The snapshot's overall confidence, derived from what it contains.
///
/// The level is the weakest among the evidence records and the impact findings, because the
/// vocabulary's aggregation rule is the minimum ordinal and a snapshot reporting a stronger
/// level than its weakest finding would be manufacturing confidence out of unrelated strong
/// evidence. The citations are every evidence identifier, so the level is never bare:
/// `confidence.schema.json` requires the array at every level including `UNKNOWN`, because
/// "we do not know" is itself evidenced.
fn overall_confidence(evidence: &[EvidenceRecord], impact: &[ImpactFinding]) -> Confidence {
    let levels: Vec<ConfidenceLevel> = evidence
        .iter()
        .map(|record| amasario_evidence::confidence::basis_for(record).level())
        .chain(impact.iter().map(|finding| finding.confidence.level))
        .collect();
    let level = if levels.is_empty() {
        ConfidenceLevel::Unknown
    } else {
        ConfidenceLevel::weakest_of(levels)
    };
    let citations: Vec<String> = evidence.iter().map(|record| record.id.clone()).collect();
    let confidence = Confidence::new(level, citations, Vec::new()).unwrap_or_else(|_| {
        // Unreachable for a snapshot, because `Capture::build` refuses an empty evidence
        // set before this is called. Stated rather than unwrapped so that a future caller
        // gets a document with a cited UNKNOWN confidence rather than a panic.
        Confidence::new(
            ConfidenceLevel::Unknown,
            vec!["the snapshot was built without evidence".to_owned()],
            Vec::new(),
        )
        .expect("a non-empty citation list")
    });
    confidence.with_rationale(format!(
        "the level is the weakest of {} evidence record(s) and {} impact finding(s) at this \
         boundary, per the vocabulary's minimum-ordinal aggregation",
        evidence.len(),
        impact.len()
    ))
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use amasario_contract::ContractExecutableKind;
    use amasario_core::{ContractId, DigestAlgorithm, NetworkType};
    use amasario_evidence::collector::{EvidenceClass, EvidenceRecord};

    /// The network every test capture is taken against.
    pub(crate) const PASSPHRASE: &str = "Test SDF Network ; September 2015";

    /// A contract address that satisfies the strkey check.
    pub(crate) const ADDRESS: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

    pub(crate) fn network() -> Network {
        Network::new("testnet", NetworkType::Testnet, PASSPHRASE).expect("a network")
    }

    pub(crate) fn boundary() -> ObservationBoundary {
        ObservationBoundary::new(
            network(),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    pub(crate) fn identity() -> ContractIdentity {
        ContractIdentity::new(
            ContractId::new(ADDRESS).expect("a contract address"),
            &network(),
            ContractExecutableKind::StellarAsset,
            None,
            boundary().ledger,
        )
        .expect("a contract identity")
    }

    /// The observation record every minimal snapshot carries.
    pub(crate) fn observation() -> EvidenceRecord {
        let mut record = EvidenceRecord::draft(
            "e-observation",
            EvidenceClass::parse("OBSERVATION"),
            "the contract was observed to exist at this boundary",
            "2026-01-01T00:00:00Z",
        );
        record.observation_note = Some("the instance entry was present".to_owned());
        record.validate().expect("the record satisfies its class");
        record
    }

    /// A snapshot with one observation record, cheap enough to build in numbers.
    pub(crate) fn minimal(captured_at: &str) -> Snapshot {
        let contract = identity();
        let boundary = boundary();
        let id = snapshot_id(&contract, &boundary);
        let mut snapshot = Snapshot {
            api_version: SUPPORTED_API_VERSION.to_owned(),
            spec_version: SUPPORTED_SPEC_VERSION.to_owned(),
            id,
            captured_at: captured_at.to_owned(),
            network: network(),
            ledger_boundary: boundary.ledger,
            boundary,
            contract,
            wasm: None,
            provenance: None,
            dependencies: None,
            graph: None,
            evidence: vec![observation()],
            confidence: Confidence::new(
                ConfidenceLevel::LowConfidence,
                vec!["e-observation".to_owned()],
                Vec::new(),
            )
            .expect("a confidence with a citation")
            .with_rationale("a single observation record at this boundary"),
            impact: Vec::new(),
            attestations: Vec::new(),
            engine_version: None,
            content_digest: Digest::new(DigestAlgorithm::Sha256, &"00".repeat(32))
                .expect("a well-formed digest"),
            volatile_fields: DEFAULT_VOLATILE_FIELDS
                .iter()
                .map(ToString::to_string)
                .collect(),
            truncated: false,
        };
        snapshot.content_digest = content_digest(&snapshot).expect("digests");
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{ADDRESS, boundary, identity, minimal, observation};
    use super::*;
    use amasario_contract::ContractExecutableKind;
    use amasario_core::{ContractId, NetworkType};
    use amasario_evidence::collector::EvidenceClass;

    #[test]
    fn a_minimal_snapshot_satisfies_every_requirement() {
        let snapshot = minimal("2026-01-01T00:00:00Z");
        assert_eq!(snapshot.failures(), Vec::new(), "{:?}", snapshot.failures());
        snapshot.validate().expect("valid");
        assert_eq!(snapshot.api_version, SUPPORTED_API_VERSION);
        assert_eq!(snapshot.spec_version, SUPPORTED_SPEC_VERSION);
        assert!(snapshot.id.starts_with(SNAPSHOT_ID_PREFIX));
        assert_eq!(snapshot.volatile_fields, ["/capturedAt", "/contentDigest"]);
    }

    #[test]
    fn the_identifier_is_derived_from_the_contract_and_the_boundary() {
        let first = minimal("2026-01-01T00:00:00Z");
        let second = minimal("2026-06-01T00:00:00Z");
        assert_eq!(
            first.id, second.id,
            "the capture time is not part of the identity"
        );

        // A different ledger is a different boundary, and therefore a different capture.
        let moved = ObservationBoundary::new(
            first.boundary.network.clone(),
            LedgerSequence::new(2_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        );
        assert_ne!(snapshot_id(&first.contract, &moved), first.id);
    }

    #[test]
    fn an_edited_identifier_is_detected() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        snapshot.id = "snapshot-chosen-by-hand".to_owned();
        let failure = snapshot
            .failures()
            .into_iter()
            .find(|failure| matches!(failure, SnapshotFailure::SnapshotIdNotStable { .. }))
            .expect("a chosen identifier is not the derived one");
        assert!(failure.to_string().contains("snapshot-chosen-by-hand"));
    }

    #[test]
    fn a_network_echo_that_disagrees_is_detected_in_every_field() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        snapshot.network = Network::new(
            "mainnet",
            NetworkType::Mainnet,
            "Public Global Stellar Network ; September 2015",
        )
        .expect("a network");
        snapshot.content_digest = content_digest(&snapshot).expect("digests");
        let disagreeing = snapshot
            .failures()
            .into_iter()
            .filter(|failure| matches!(failure, SnapshotFailure::BoundaryEchoDisagrees { .. }))
            .count();
        assert_eq!(
            disagreeing, 2,
            "the identifier and the passphrase both disagree"
        );
    }

    #[test]
    fn a_ledger_boundary_that_disagrees_is_detected() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        snapshot.ledger_boundary = LedgerSequence::new(2_000).expect("a real ledger");
        snapshot.content_digest = content_digest(&snapshot).expect("digests");
        assert!(snapshot.failures().iter().any(|failure| matches!(
            failure,
            SnapshotFailure::BoundaryEchoDisagrees { field, .. } if field == "/ledgerBoundary"
        )));
    }

    #[test]
    fn a_snapshot_with_no_evidence_is_refused_rather_than_padded() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        snapshot.evidence.clear();
        assert!(
            snapshot
                .failures()
                .iter()
                .any(|failure| matches!(failure, SnapshotFailure::NoEvidenceRecorded))
        );
    }

    #[test]
    fn a_capture_refuses_to_build_without_evidence() {
        let capture = Capture::of(identity(), boundary());
        let error = capture
            .build("2026-01-01T00:00:00Z")
            .expect_err("an empty evidence set is not a snapshot");
        assert!(error.to_string().contains("no evidence"), "got: {error}");
    }

    #[test]
    fn duplicate_evidence_identifiers_are_refused_at_collection() {
        let mut capture = Capture::of(identity(), boundary());
        let record = observation();
        capture
            .add_evidence(record.clone())
            .expect("the first copy is accepted");
        let error = capture
            .add_evidence(record)
            .expect_err("a duplicate identifier makes every citation ambiguous");
        assert!(
            error.to_string().contains("share the identifier"),
            "got: {error}"
        );
        assert_eq!(capture.evidence.len(), 1);

        let snapshot = capture
            .build("2026-01-01T00:00:00Z")
            .expect("one record is enough");
        assert_eq!(snapshot.evidence_ids(), ["e-observation"]);
    }

    #[test]
    fn a_capture_normalises_what_it_collects_and_digests_the_normalised_form() {
        let mut secondary = observation();
        secondary.id = "e-0".to_owned();
        let mut capture = Capture::of(identity(), boundary())
            .with_engine_version("1.0.0")
            .truncated(true);
        // Collected out of canonical order on purpose: the builder normalises, so the
        // order they arrive in must not reach the document.
        capture
            .add_evidence(observation())
            .expect("a distinct identifier");
        capture
            .add_evidence(secondary)
            .expect("a distinct identifier");
        let snapshot = capture.build("2026-01-01T00:00:00Z").expect("a snapshot");
        assert_eq!(
            snapshot.evidence_ids(),
            ["e-0", "e-observation"],
            "the collection order must not reach the document"
        );
        assert_eq!(snapshot.engine_version.as_deref(), Some("1.0.0"));
        assert!(snapshot.truncated);
        verify_content_digest(&snapshot).expect("the digest it computed is the one it recorded");
    }

    #[test]
    fn the_overall_confidence_is_the_weakest_part_of_the_snapshot() {
        let snapshot = minimal("2026-01-01T00:00:00Z");
        // The single observation record is circumstantial, the weakest basis, so the
        // snapshot cannot claim more than that however strong its other sections are.
        assert_eq!(snapshot.weakest_level(), ConfidenceLevel::LowConfidence);
        assert_eq!(snapshot.confidence.level, ConfidenceLevel::LowConfidence);
        assert!(snapshot.confidence.rationale.is_some());
        assert_eq!(snapshot.confidence.evidence, ["e-observation"]);
    }

    #[test]
    fn a_snapshot_from_another_specification_version_is_refused() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        snapshot.spec_version = "2.0.0".to_owned();
        snapshot.content_digest = content_digest(&snapshot).expect("digests");
        assert!(snapshot.failures().iter().any(|failure| matches!(
            failure,
            SnapshotFailure::SpecVersionIncompatible { found, .. } if found == "2.0.0"
        )));
    }

    #[test]
    fn a_contract_identity_whose_executable_kind_disagrees_with_its_hash_is_refused_earlier() {
        // The identity model already refuses this, which is why the snapshot does not
        // re-check it: a rule implemented twice is a rule that can disagree with itself.
        let error = ContractIdentity::new(
            ContractId::new(ADDRESS).expect("a contract address"),
            &tests_support::network(),
            ContractExecutableKind::Wasm,
            None,
            boundary().ledger,
        )
        .expect_err("a WASM contract without a hash has no identity");
        assert!(!error.to_string().is_empty());
        let _ = EvidenceClass::parse("OBSERVATION");
    }
}
