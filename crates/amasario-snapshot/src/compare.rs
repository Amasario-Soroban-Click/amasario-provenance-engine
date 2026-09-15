//! Comparing two snapshots into a machine-readable difference.
//!
//! # Why a diff is the artefact with the least room for slack
//!
//! `rules/impact/change-impact` states the requirement in a single sentence: a diff "MUST be
//! produced by canonical comparison, MUST NOT report a difference caused only by ordering or
//! by a volatile field, MUST classify every difference with a category and a change type,
//! and MUST state a reason for each". The rationale is stated in the same rule and is worth
//! repeating because it explains every decision below: a diff "is the artefact most likely
//! to be consumed without review, in a pipeline that fails or alerts on any entry, so a
//! single spurious entry makes the whole comparison untrustworthy".
//!
//! Three consequences follow.
//!
//! **Comparison is canonical or it is not a result.** [`compare`] normalises both snapshots
//! before looking at them, and [`ComparisonMode`] records which mode was used. The other two
//! modes exist so that an implementation that compared differently can say so rather than
//! present its result as canonical - and [`Diff::failures`] refuses a diff produced in
//! either of them, because the rule requires canonical comparison of the published result.
//!
//! **An incomparable pair is reported as incomparable.** An empty diff is indistinguishable
//! from "nothing changed", so two snapshots from different networks, different specification
//! families, or in the wrong ledger order are reported with `comparable: false`, a reason
//! from the schema's closed set, and no changes at all. Guessing that they are merely equal
//! is the one answer the rule forbids.
//!
//! **Ordering never appears as a change.** Every set is normalised before it is compared, so
//! a re-capture that merely reordered evidence produces no entries - which is the common
//! case, and the one that would otherwise make every run alert.
//!
//! # Why the summary is stored rather than derived on read
//!
//! `schema/diff.schema.json` records counts "so that the summary cannot disagree with the
//! changes array", and the rule requires them to agree. Storing counts that a consumer can
//! check against the array is stronger than computing them at read time: a consumer
//! filtering on the array and counting the summary would otherwise be reading two documents'
//! worth of truth without knowing it.

use amasario_core::{
    Digest, EntityKind, EntityRef, LedgerSequence, Result, SUPPORTED_API_VERSION,
    SUPPORTED_SPEC_VERSION,
};
use amasario_impact::ChangeType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::Snapshot;
use crate::errors::SnapshotFailure;
use crate::normalize::{canonicalise, entity_key};

/// The shortest a reason may be before it stops being one.
///
/// Matching the evidence layer's minimum: a reason exists so that a reviewer can reject an
/// entry that turns out to be noise, and a two-word label does not give them anything to
/// reject it on.
pub const MINIMUM_REASON_LENGTH: usize = 8;

/// How two snapshots were compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComparisonMode {
    /// Every set is normalised before comparing. The only deterministic mode.
    Canonical,
    /// The order entities appear in was treated as significant.
    OrderSensitive,
    /// Only differences that survive every ordering were reported, without normalising.
    OrderInsensitiveOnly,
}

impl ComparisonMode {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => "CANONICAL",
            Self::OrderSensitive => "ORDER_SENSITIVE",
            Self::OrderInsensitiveOnly => "ORDER_INSENSITIVE_ONLY",
        }
    }

    /// Whether a diff produced in this mode is a publishable result.
    ///
    /// Only canonical comparison is: the other two are recorded so that an implementation
    /// that compared differently says so, and neither is deterministic enough to alert on.
    #[must_use]
    pub const fn is_deterministic(self) -> bool {
        matches!(self, Self::Canonical)
    }
}

/// Why two snapshots could not be compared.
///
/// The set is closed and is the schema's own, because a consumer subscribing to diffs needs
/// to handle every reason exhaustively rather than to read prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IncomparableReason {
    /// The documents belong to different major-version families.
    ApiVersionMismatch,
    /// The specification versions cannot be interpreted together.
    SpecVersionIncompatible,
    /// The snapshots describe different chains.
    NetworkMismatch,
    /// The earlier snapshot is bounded at a later ledger than the later one.
    BoundaryOrderInvalid,
    /// At least one document could not be read as a snapshot.
    MalformedSnapshot,
}

impl IncomparableReason {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiVersionMismatch => "API_VERSION_MISMATCH",
            Self::SpecVersionIncompatible => "SPEC_VERSION_INCOMPATIBLE",
            Self::NetworkMismatch => "NETWORK_MISMATCH",
            Self::BoundaryOrderInvalid => "BOUNDARY_ORDER_INVALID",
            Self::MalformedSnapshot => "MALFORMED_SNAPSHOT",
        }
    }
}

/// What kind of thing changed.
///
/// The categories are the schema's closed enumeration, and the reason each exists is that a
/// consumer subscribes to a category rather than parsing a field path. `IMPACT_SURFACE_CHANGED`
/// is the one a pipeline most often acts on; `TRUNCATION_CHANGED` is the one it most often
/// ignores, which is why it is a category of its own rather than a note on another entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChangeCategory {
    /// A relationship exists in the after-state and not in the before-state.
    RelationshipAdded,
    /// A relationship existed in the before-state and not in the after-state.
    RelationshipRemoved,
    /// A relationship exists in both and at least one of its properties differs.
    RelationshipChanged,
    /// The provenance record differs.
    ProvenanceChanged,
    /// The deployed executable's identity differs.
    WasmIdentityChanged,
    /// What put the contract instance into its present state differs.
    DeploymentChanged,
    /// The evidence set differs.
    EvidenceChanged,
    /// The confidence differs.
    ConfidenceChanged,
    /// The set of impact findings differs.
    ImpactSurfaceChanged,
    /// The contract identity itself differs.
    ContractIdentityChanged,
    /// Whether the snapshot was bounded differs.
    TruncationChanged,
}

impl ChangeCategory {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RelationshipAdded => "RELATIONSHIP_ADDED",
            Self::RelationshipRemoved => "RELATIONSHIP_REMOVED",
            Self::RelationshipChanged => "RELATIONSHIP_CHANGED",
            Self::ProvenanceChanged => "PROVENANCE_CHANGED",
            Self::WasmIdentityChanged => "WASM_IDENTITY_CHANGED",
            Self::DeploymentChanged => "DEPLOYMENT_CHANGED",
            Self::EvidenceChanged => "EVIDENCE_CHANGED",
            Self::ConfidenceChanged => "CONFIDENCE_CHANGED",
            Self::ImpactSurfaceChanged => "IMPACT_SURFACE_CHANGED",
            Self::ContractIdentityChanged => "CONTRACT_IDENTITY_CHANGED",
            Self::TruncationChanged => "TRUNCATION_CHANGED",
        }
    }

    /// The order categories are reported in.
    ///
    /// Identity first, then the chain from source to deployment, then the relationships
    /// derived from it, then the analysis over those. That is the order a reader forms the
    /// picture in, and fixing it here means two runs of the same comparison write the same
    /// document rather than a document that depends on the iteration order.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::ContractIdentityChanged => 0,
            Self::WasmIdentityChanged => 1,
            Self::ProvenanceChanged => 2,
            Self::DeploymentChanged => 3,
            Self::EvidenceChanged => 4,
            Self::RelationshipAdded => 5,
            Self::RelationshipRemoved => 6,
            Self::RelationshipChanged => 7,
            Self::ConfidenceChanged => 8,
            Self::ImpactSurfaceChanged => 9,
            Self::TruncationChanged => 10,
        }
    }

    /// Whether a change in this category should trigger impact re-analysis.
    ///
    /// Recorded by the producer so that "every consumer does not have to redefine which
    /// categories matter", in the schema's own words. Identity and relationship changes do:
    /// they are what the impact layer propagates over. A confidence change on an unchanged
    /// graph does not, because the graph it would propagate over is the same one.
    #[must_use]
    pub const fn is_impact_relevant(self) -> bool {
        matches!(
            self,
            Self::RelationshipAdded
                | Self::RelationshipRemoved
                | Self::RelationshipChanged
                | Self::WasmIdentityChanged
                | Self::ProvenanceChanged
                | Self::DeploymentChanged
                | Self::ContractIdentityChanged
        )
    }
}

/// One of the snapshots being compared, identified rather than embedded.
///
/// `schema/diff.schema.json` requires a reference rather than an embedded document "so that
/// a diff cannot be read without the snapshots it compares being available". That is a
/// deliberate inconvenience: a diff read on its own is a list of assertions about documents
/// nobody has, and the digest is what lets a reader confirm they are holding the same bytes
/// the producer compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRef {
    /// The snapshot's identifier.
    pub id: String,
    /// The digest of the snapshot it refers to.
    #[serde(rename = "contentDigest")]
    pub content_digest: Digest,
    /// The ledger the snapshot was bounded at.
    #[serde(rename = "ledgerBoundary", skip_serializing_if = "Option::is_none")]
    pub ledger_boundary: Option<LedgerSequence>,
    /// The specification version that produced it.
    #[serde(rename = "specVersion", skip_serializing_if = "Option::is_none")]
    pub spec_version: Option<String>,
    /// The engine version that produced it.
    #[serde(rename = "engineVersion", skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
}

impl SnapshotRef {
    /// Refers to a snapshot.
    #[must_use]
    pub fn of(snapshot: &Snapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            content_digest: snapshot.content_digest.clone(),
            ledger_boundary: Some(snapshot.ledger_boundary),
            spec_version: Some(snapshot.spec_version.clone()),
            engine_version: snapshot.engine_version.clone(),
        }
    }
}

/// One difference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffEntry {
    /// The change's identifier, deterministic given the same inputs.
    pub id: String,
    /// What kind of thing changed.
    pub category: ChangeCategory,
    /// How it changed.
    #[serde(rename = "changeType")]
    pub change_type: ChangeType,
    /// The entity the difference concerns.
    pub entity: EntityRef,
    /// The value in the earlier snapshot. Absent for an addition.
    #[serde(rename = "beforeValue", skip_serializing_if = "Option::is_none")]
    pub before_value: Option<Value>,
    /// The value in the later snapshot. Absent for a removal.
    #[serde(rename = "afterValue", skip_serializing_if = "Option::is_none")]
    pub after_value: Option<Value>,
    /// The JSON pointer at which a field-level difference was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Why this is a difference rather than a serialisation artefact.
    pub reason: String,
    /// Whether this change should trigger impact re-analysis.
    #[serde(rename = "impactRelevant")]
    pub impact_relevant: bool,
}

impl DiffEntry {
    /// Builds an entry, deriving its identifier from what it describes.
    ///
    /// The identifier is `<category>/<entity kind>:<entity id>[/<path>]`, which is
    /// deterministic given the same inputs and stable under a reordering of the document -
    /// so a diff can itself be diffed, which is what the schema asks the identifier for.
    ///
    /// The reason is accepted as given and checked by [`Diff::failures`]. Checking it here
    /// as well would be the same rule implemented twice, and a rule implemented twice is a
    /// rule that can disagree with itself: an entry could satisfy this constructor and fail
    /// the diff-level check, or the reverse, and neither would be the requirement.
    #[must_use]
    pub fn new(
        category: ChangeCategory,
        change_type: ChangeType,
        entity: EntityRef,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id: change_id(category, &entity, None),
            category,
            change_type,
            entity,
            before_value: None,
            after_value: None,
            path: None,
            reason: reason.into(),
            impact_relevant: category.is_impact_relevant(),
        }
    }

    /// Records the value before the change.
    #[must_use]
    pub fn with_before(mut self, value: Option<Value>) -> Self {
        self.before_value = value;
        self
    }

    /// Records the value after the change.
    #[must_use]
    pub fn with_after(mut self, value: Option<Value>) -> Self {
        self.after_value = value;
        self
    }

    /// Records the JSON pointer the difference was found at.
    ///
    /// Recording the path changes the identifier, because two differences of one category
    /// against one entity at different paths are two differences: without the path the
    /// second would collide with the first and a duplicate identifier is refused.
    #[must_use]
    pub fn at_path(mut self, path: impl Into<String>) -> Self {
        let path = path.into();
        self.id = change_id(self.category, &self.entity, Some(&path));
        self.path = Some(path);
        self
    }

    /// The order entries are reported in.
    #[must_use]
    pub fn canonical_key(&self) -> (u8, String) {
        let (kind, id) = entity_key(&self.entity);
        (
            self.category.rank(),
            format!(
                "{id}\u{1f}{kind}\u{1f}{}",
                self.path.as_deref().unwrap_or("")
            ),
        )
    }
}

/// The identifier a difference carries.
fn change_id(category: ChangeCategory, entity: &EntityRef, path: Option<&str>) -> String {
    let (kind, id) = entity_key(entity);
    match path {
        Some(path) => format!("change:{}/{}:{}{path}", category.as_str(), kind, id),
        None => format!("change:{}/{}:{}", category.as_str(), kind, id),
    }
}

/// Counts by category and change type, with the total.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffSummary {
    /// How many differences were reported.
    pub total: usize,
    /// How many of each category.
    #[serde(rename = "byCategory", default, skip_serializing_if = "Vec::is_empty")]
    pub by_category: Vec<(String, usize)>,
    /// How many of each change type.
    #[serde(
        rename = "byChangeType",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub by_change_type: Vec<(String, usize)>,
}

impl DiffSummary {
    /// Counts a set of entries.
    ///
    /// `BTreeMap` rather than `HashMap`, so the counts are written in name order rather than
    /// in an order that varies between runs: a summary that differed between two runs of one
    /// comparison would itself be a difference a diff reported.
    #[must_use]
    pub fn of(changes: &[DiffEntry]) -> Self {
        let mut by_category = std::collections::BTreeMap::new();
        let mut by_change_type = std::collections::BTreeMap::new();
        for change in changes {
            *by_category
                .entry(change.category.as_str().to_owned())
                .or_insert(0) += 1;
            *by_change_type
                .entry(change.change_type.as_str().to_owned())
                .or_insert(0) += 1;
        }
        Self {
            total: changes.len(),
            by_category: by_category.into_iter().collect(),
            by_change_type: by_change_type.into_iter().collect(),
        }
    }
}

/// The difference between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diff {
    /// The specification's `apiVersion` for the family this document belongs to.
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    /// The normative specification version that produced the document.
    #[serde(rename = "specVersion")]
    pub spec_version: String,
    /// The earlier snapshot.
    pub before: SnapshotRef,
    /// The later snapshot.
    pub after: SnapshotRef,
    /// How the two were compared.
    #[serde(rename = "comparisonMode")]
    pub comparison_mode: ComparisonMode,
    /// Whether the two were comparable at all.
    pub comparable: bool,
    /// Why they were not, when they were not.
    #[serde(rename = "incomparableReason", skip_serializing_if = "Option::is_none")]
    pub incomparable_reason: Option<IncomparableReason>,
    /// Every difference detected.
    #[serde(default)]
    pub changes: Vec<DiffEntry>,
    /// Counts that must agree with `changes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<DiffSummary>,
    /// When the comparison ran.
    #[serde(rename = "generatedAt", skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

impl Diff {
    /// Every way this diff disagrees with what the rule requires of it.
    #[must_use]
    pub fn failures(&self) -> Vec<SnapshotFailure> {
        let mut failures: Vec<SnapshotFailure> = Vec::new();

        if !self.comparison_mode.is_deterministic() {
            failures.push(SnapshotFailure::ComparisonModeNotCanonical {
                mode: self.comparison_mode.as_str().to_owned(),
            });
        }
        if !self.comparable {
            let reason = self
                .incomparable_reason
                .map_or("an unstated reason".to_owned(), |reason| {
                    reason.as_str().to_owned()
                });
            failures.push(SnapshotFailure::Incomparable { reason });
            if !self.changes.is_empty() {
                failures.push(SnapshotFailure::ChangesWithIncomparable {
                    changes: self.changes.len(),
                });
            }
        }

        let mut seen: Vec<&str> = Vec::new();
        for change in &self.changes {
            if change.reason.chars().count() < MINIMUM_REASON_LENGTH {
                failures.push(SnapshotFailure::ReasonNotStated {
                    change: change.id.clone(),
                });
            }
            if seen.contains(&change.id.as_str()) {
                failures.push(SnapshotFailure::DuplicateChangeId {
                    id: change.id.clone(),
                });
            }
            seen.push(&change.id);
        }

        if let Some(summary) = &self.summary
            && summary.total != self.changes.len()
        {
            failures.push(SnapshotFailure::SummaryDisagrees {
                claimed: summary.total,
                actual: self.changes.len(),
            });
        }

        failures
    }

    /// Whether the diff satisfies every requirement.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.failures().is_empty()
    }

    /// Validates the diff, reporting the first failure.
    ///
    /// # Errors
    ///
    /// Returns the first failure from [`Self::failures`].
    pub fn validate(&self) -> Result<()> {
        crate::errors::first_failure(self.failures())
    }

    /// The categories that appear, in report order.
    #[must_use]
    pub fn categories(&self) -> Vec<ChangeCategory> {
        let mut categories: Vec<ChangeCategory> =
            self.changes.iter().map(|change| change.category).collect();
        categories.sort_by_key(|category| category.rank());
        categories.dedup();
        categories
    }

    /// Whether any change should trigger impact re-analysis.
    #[must_use]
    pub fn triggers_impact(&self) -> bool {
        self.changes.iter().any(|change| change.impact_relevant)
    }
}

/// Compares two snapshots canonically.
///
/// # Errors
///
/// Returns a snapshot error only when a sub-document cannot be serialised for comparison,
/// which would be a defect in one of the producing crates rather than in the input. An
/// incomparable pair is not an error: it is a diff with `comparable: false`, because a
/// caller that received an error could not tell it from a transport failure.
pub fn compare(
    before: &Snapshot,
    after: &Snapshot,
    generated_at: impl Into<String>,
) -> Result<Diff> {
    let mut before = before.clone();
    let mut after = after.clone();
    canonicalise(&mut before);
    canonicalise(&mut after);

    let mut diff = Diff {
        api_version: SUPPORTED_API_VERSION.to_owned(),
        spec_version: SUPPORTED_SPEC_VERSION.to_owned(),
        before: SnapshotRef::of(&before),
        after: SnapshotRef::of(&after),
        comparison_mode: ComparisonMode::Canonical,
        comparable: true,
        incomparable_reason: None,
        changes: Vec::new(),
        summary: None,
        generated_at: Some(generated_at.into()),
    };

    if let Some(reason) = incomparability(&before, &after) {
        diff.comparable = false;
        diff.incomparable_reason = Some(reason);
        diff.summary = Some(DiffSummary::default());
        return Ok(diff);
    }

    let mut changes: Vec<DiffEntry> = Vec::new();
    contract_identity_changes(&before, &after, &mut changes)?;
    wasm_changes(&before, &after, &mut changes)?;
    provenance_changes(&before, &after, &mut changes)?;
    deployment_changes(&before, &after, &mut changes)?;
    evidence_changes(&before, &after, &mut changes)?;
    relationship_changes(&before, &after, &mut changes)?;
    confidence_changes(&before, &after, &mut changes);

    if before.truncated != after.truncated {
        changes.push(
            DiffEntry::new(
                ChangeCategory::TruncationChanged,
                ChangeType::Modified,
                contract_entity(&after),
                format!(
                    "the earlier capture was {} bounded and the later one is {}, so the two \
                     results are not comparable in coverage however similar their contents are",
                    if before.truncated { "" } else { "not" },
                    if after.truncated { "" } else { "not" }
                ),
            )
            .with_before(Some(Value::Bool(before.truncated)))
            .with_after(Some(Value::Bool(after.truncated))),
        );
    }
    impact_changes(&before, &after, &mut changes);

    changes.sort_by_key(DiffEntry::canonical_key);
    diff.summary = Some(DiffSummary::of(&changes));
    diff.changes = changes;
    Ok(diff)
}

/// Why two snapshots cannot be compared, if they cannot.
fn incomparability(before: &Snapshot, after: &Snapshot) -> Option<IncomparableReason> {
    if before.api_version != after.api_version {
        return Some(IncomparableReason::ApiVersionMismatch);
    }
    if before.spec_version != after.spec_version {
        return Some(IncomparableReason::SpecVersionIncompatible);
    }
    // The passphrase identifies the chain; the identifier is what an operator calls it. Two
    // operators can name one chain differently, so only a passphrase difference means the
    // snapshots describe different networks.
    if before.network.passphrase != after.network.passphrase {
        return Some(IncomparableReason::NetworkMismatch);
    }
    if before.ledger_boundary.get() > after.ledger_boundary.get() {
        return Some(IncomparableReason::BoundaryOrderInvalid);
    }
    None
}

/// The contract entity a snapshot is about.
fn contract_entity(snapshot: &Snapshot) -> EntityRef {
    EntityRef::new(EntityKind::Contract, snapshot.contract.contract_id.as_str())
        .expect("a contract address is a non-empty identifier")
}

/// Records one field-level difference, where the values differ.
///
/// The comparison is on the serialised form rather than on the typed values, because the
/// question a diff answers is whether the document changed rather than whether a field a
/// reader considers important changed. `serde_json::Value` equality is the same test a
/// consumer recomputing the digest performs.
fn field_change(
    changes: &mut Vec<DiffEntry>,
    category: ChangeCategory,
    entity: EntityRef,
    path: &str,
    before: &Value,
    after: &Value,
    reason: String,
) {
    if before == after {
        return;
    }
    changes.push(
        DiffEntry::new(category, ChangeType::Modified, entity, reason)
            .with_before(Some(before.clone()))
            .with_after(Some(after.clone()))
            .at_path(path),
    );
}

/// Serialises a sub-document for comparison.
fn value_of<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| {
        SnapshotFailure::Malformed {
            source: "a snapshot section being compared".to_owned(),
            detail: error.to_string(),
        }
        .into_error()
    })
}

fn contract_identity_changes(
    before: &Snapshot,
    after: &Snapshot,
    changes: &mut Vec<DiffEntry>,
) -> Result<()> {
    let entity = contract_entity(after);
    let before_value = value_of(&before.contract)?;
    let after_value = value_of(&after.contract)?;
    // The identifier the snapshot is about is compared before the rest of the identity,
    // because a contract address change means the two documents are not about the same
    // thing and the remaining differences are then between two different subjects.
    if before.contract.contract_id != after.contract.contract_id {
        changes.push(
            DiffEntry::new(
                ChangeCategory::ContractIdentityChanged,
                ChangeType::Replaced,
                entity.clone(),
                format!(
                    "the contract address changed from {} to {}, so the two captures describe \
                     different contracts rather than different states of one",
                    before.contract.contract_id, after.contract.contract_id
                ),
            )
            .with_before(Some(Value::String(before.contract.contract_id.to_string())))
            .with_after(Some(Value::String(after.contract.contract_id.to_string())))
            .at_path("/contract/contractId"),
        );
    }
    field_change(
        changes,
        ChangeCategory::ContractIdentityChanged,
        entity,
        "/contract",
        &before_value,
        &after_value,
        "the contract identity's non-address fields differ, which means the description of \
         what the address executes or when it was observed changed"
            .to_owned(),
    );
    Ok(())
}

fn wasm_changes(before: &Snapshot, after: &Snapshot, changes: &mut Vec<DiffEntry>) -> Result<()> {
    let entity = EntityRef::new(EntityKind::Contract, after.contract.contract_id.as_str())
        .expect("a contract address is a non-empty identifier");
    let before_value = value_of(&before.wasm)?;
    let after_value = value_of(&after.wasm)?;
    if before_value == after_value {
        return Ok(());
    }
    // A changed executable hash is the change Amasario exists to make visible, so it gets
    // REPLACED rather than MODIFIED: there is no ordering between two module hashes, and
    // `taxonomies/change-types.yaml` names this exact case as REPLACED's primary example.
    let change_type = match (&before.wasm, &after.wasm) {
        (Some(_), Some(_)) => ChangeType::Replaced,
        (None, Some(_)) => ChangeType::Added,
        (Some(_), None) => ChangeType::Removed,
        (None, None) => return Ok(()),
    };
    changes.push(
        DiffEntry::new(
            ChangeCategory::WasmIdentityChanged,
            change_type,
            entity,
            "the deployed executable's identity changed, and Amasario does not order two \
             module hashes, so the change is reported as a replacement rather than as an \
             upgrade or a downgrade"
                .to_owned(),
        )
        .with_before(Some(before_value))
        .with_after(Some(after_value))
        .at_path("/wasm"),
    );
    Ok(())
}

fn provenance_changes(
    before: &Snapshot,
    after: &Snapshot,
    changes: &mut Vec<DiffEntry>,
) -> Result<()> {
    if before.provenance.is_none() && after.provenance.is_none() {
        return Ok(());
    }
    let entity = contract_entity(after);
    let before_value = value_of(&before.provenance)?;
    let after_value = value_of(&after.provenance)?;
    if before_value == after_value {
        return Ok(());
    }
    let change_type = match (&before.provenance, &after.provenance) {
        (Some(_), Some(_)) => ChangeType::Modified,
        (None, Some(_)) => ChangeType::Added,
        (Some(_), None) => ChangeType::Removed,
        (None, None) => return Ok(()),
    };
    changes.push(
        DiffEntry::new(
            ChangeCategory::ProvenanceChanged,
            change_type,
            entity,
            "the provenance record differs, so either the links that were established changed \
             or the evidence and verification state of one of them did"
                .to_owned(),
        )
        .with_before(Some(before_value))
        .with_after(Some(after_value))
        .at_path("/provenance"),
    );
    Ok(())
}

fn deployment_changes(
    before: &Snapshot,
    after: &Snapshot,
    changes: &mut Vec<DiffEntry>,
) -> Result<()> {
    let before_value = value_of(&before.contract.instance_modification)?;
    let after_value = value_of(&after.contract.instance_modification)?;
    if before_value == after_value {
        return Ok(());
    }
    let entity = EntityRef::new(EntityKind::Deployment, after.contract.contract_id.as_str())
        .expect("a contract address is a non-empty identifier");
    changes.push(
        DiffEntry::new(
            ChangeCategory::DeploymentChanged,
            ChangeType::Modified,
            entity,
            "the operation that put the contract's instance entry into its present state \
             changed, which is what a re-deployment or an upgrade leaves behind"
                .to_owned(),
        )
        .with_before(Some(before_value))
        .with_after(Some(after_value))
        .at_path("/contract/instanceModification"),
    );
    Ok(())
}

fn evidence_changes(
    before: &Snapshot,
    after: &Snapshot,
    changes: &mut Vec<DiffEntry>,
) -> Result<()> {
    let entity = contract_entity(after);
    let before_values: Vec<(String, Value)> = before
        .evidence
        .iter()
        .map(|record| Ok((record.id.clone(), value_of(record)?)))
        .collect::<Result<Vec<_>>>()?;
    let after_values: Vec<(String, Value)> = after
        .evidence
        .iter()
        .map(|record| Ok((record.id.clone(), value_of(record)?)))
        .collect::<Result<Vec<_>>>()?;

    for (id, after_value) in &after_values {
        match before_values.iter().find(|(before_id, _)| before_id == id) {
            None => changes.push(
                DiffEntry::new(
                    ChangeCategory::EvidenceChanged,
                    ChangeType::Added,
                    entity.clone(),
                    format!(
                        "evidence record {id:?} is present in the later capture and was not in \
                         the earlier one, so a claim it supports is newly supported"
                    ),
                )
                .with_after(Some(after_value.clone()))
                .at_path(element_path("evidence", id)),
            ),
            Some((_, before_value)) if before_value != after_value => changes.push(
                DiffEntry::new(
                    ChangeCategory::EvidenceChanged,
                    ChangeType::Modified,
                    entity.clone(),
                    format!(
                        "evidence record {id:?} exists in both captures with different contents, \
                         and a record's contents are what a reviewer checks it against"
                    ),
                )
                .with_before(Some(before_value.clone()))
                .with_after(Some(after_value.clone()))
                .at_path(element_path("evidence", id)),
            ),
            Some(_) => {},
        }
    }
    for (id, before_value) in &before_values {
        if !after_values.iter().any(|(after_id, _)| after_id == id) {
            changes.push(
                DiffEntry::new(
                    ChangeCategory::EvidenceChanged,
                    ChangeType::Removed,
                    entity.clone(),
                    format!(
                        "evidence record {id:?} was present in the earlier capture and is absent \
                         from the later one, so a claim that rested on it no longer does"
                    ),
                )
                .with_before(Some(before_value.clone()))
                .at_path(element_path("evidence", id)),
            );
        }
    }
    Ok(())
}

/// The path an element of a collection is reported at.
///
/// The element's identity belongs in the path because the path is part of the change
/// identifier. Two additions to one collection are two changes, and reporting both at a
/// bare `/impact` would give them one identifier - which the diff's own duplicate rule
/// then refuses, leaving no diff at all. The segment is escaped as RFC 6901 requires, so
/// that an identifier containing a slash cannot invent a level of nesting.
fn element_path(collection: &str, element: &str) -> String {
    let escaped = element.replace('~', "~0").replace('/', "~1");
    format!("/{collection}/{escaped}")
}

/// The comparable triple one relationship asserts.
fn triple_of(dependency: &amasario_dependency::Dependency) -> String {
    let (subject_kind, subject_id) = entity_key(&dependency.subject);
    let (object_kind, object_id) = entity_key(&dependency.object);
    format!(
        "{subject_kind}:{subject_id} -{}-> {object_kind}:{object_id}",
        dependency.relationship.as_str()
    )
}

/// The relationship triples a snapshot asserts, as comparable strings.
///
/// Drawn from the dependency set rather than the graph, because the dependency set is what
/// carries the classification, confidence and evidence of each relationship; the graph is a
/// projection of it. A snapshot that recorded only a graph would compare the projection and
/// call a confidence change a new relationship.
fn relationship_triples(snapshot: &Snapshot) -> Vec<String> {
    snapshot
        .dependencies
        .as_ref()
        .map_or_else(Vec::new, |dependencies| {
            let mut triples: Vec<String> = dependencies.all().map(triple_of).collect();
            triples.sort();
            triples.dedup();
            triples
        })
}

fn relationship_changes(
    before: &Snapshot,
    after: &Snapshot,
    changes: &mut Vec<DiffEntry>,
) -> Result<()> {
    let before_triples = relationship_triples(before);
    let after_triples = relationship_triples(after);
    let entity = contract_entity(after);

    for triple in &after_triples {
        if !before_triples.contains(triple) {
            changes.push(
                DiffEntry::new(
                    ChangeCategory::RelationshipAdded,
                    ChangeType::Added,
                    entity.clone(),
                    format!(
                        "the relationship {triple} is asserted in the later capture and was not \
                         asserted in the earlier one"
                    ),
                )
                .with_after(Some(Value::String(triple.clone())))
                .at_path(element_path("dependencies", triple)),
            );
        }
    }
    for triple in &before_triples {
        if !after_triples.contains(triple) {
            changes.push(
                DiffEntry::new(
                    ChangeCategory::RelationshipRemoved,
                    ChangeType::Removed,
                    entity.clone(),
                    format!(
                        "the relationship {triple} was asserted in the earlier capture and is not \
                         asserted in the later one"
                    ),
                )
                .with_before(Some(Value::String(triple.clone())))
                .at_path(element_path("dependencies", triple)),
            );
        }
    }

    // A relationship present in both whose confidence or evidence changed is a change to the
    // relationship rather than to its existence, and it is reported separately so that a
    // consumer filtering on additions does not see a strengthened edge as a new one.
    let before_dependencies = before.dependencies.as_ref();
    let Some(after_dependencies) = after.dependencies.as_ref() else {
        return Ok(());
    };
    for dependency in after_dependencies.all() {
        let Some(earlier) = before_dependencies.and_then(|dependencies| {
            dependencies.all().find(|earlier| {
                earlier.subject == dependency.subject
                    && earlier.object == dependency.object
                    && earlier.relationship == dependency.relationship
            })
        }) else {
            continue;
        };
        if earlier.confidence == dependency.confidence
            && earlier.verification == dependency.verification
            && earlier.classes == dependency.classes
        {
            continue;
        }
        changes.push(
            DiffEntry::new(
                ChangeCategory::RelationshipChanged,
                ChangeType::Modified,
                dependency.object.clone(),
                format!(
                    "the relationship from {} to {} survived the change but its confidence, \
                     verification state or classification did not, so a conclusion resting on \
                     it must be revisited",
                    dependency.subject.id, dependency.object.id
                ),
            )
            .with_before(Some(value_of(earlier)?))
            .with_after(Some(value_of(dependency)?))
            .at_path(element_path("dependencies", &triple_of(dependency))),
        );
    }
    Ok(())
}

fn confidence_changes(before: &Snapshot, after: &Snapshot, changes: &mut Vec<DiffEntry>) {
    if before.confidence.level == after.confidence.level {
        return;
    }
    changes.push(
        DiffEntry::new(
            ChangeCategory::ConfidenceChanged,
            ChangeType::Modified,
            contract_entity(after),
            format!(
                "the snapshot's overall confidence moved from {} to {}, which is a statement \
                 about how much evidence there is rather than about the contract",
                before.confidence.level.as_str(),
                after.confidence.level.as_str()
            ),
        )
        .with_before(Some(Value::String(
            before.confidence.level.as_str().to_owned(),
        )))
        .with_after(Some(Value::String(
            after.confidence.level.as_str().to_owned(),
        )))
        .at_path("/confidence/level"),
    );
}

fn impact_changes(before: &Snapshot, after: &Snapshot, changes: &mut Vec<DiffEntry>) {
    let before_ids: Vec<&str> = before
        .impact
        .iter()
        .map(|finding| finding.id.as_str())
        .collect();
    let after_ids: Vec<&str> = after
        .impact
        .iter()
        .map(|finding| finding.id.as_str())
        .collect();
    let entity = contract_entity(after);

    for finding in &after.impact {
        if !before_ids.contains(&finding.id.as_str()) {
            changes.push(
                DiffEntry::new(
                    ChangeCategory::ImpactSurfaceChanged,
                    ChangeType::Added,
                    entity.clone(),
                    format!(
                        "finding {} is new in the later capture, so the set of entities a change \
                         could reach is larger than it was",
                        finding.id
                    ),
                )
                .with_after(Some(Value::String(finding.id.clone())))
                .at_path(element_path("impact", &finding.id)),
            );
        }
    }
    for finding in &before.impact {
        if !after_ids.contains(&finding.id.as_str()) {
            changes.push(
                DiffEntry::new(
                    ChangeCategory::ImpactSurfaceChanged,
                    ChangeType::Removed,
                    entity.clone(),
                    format!(
                        "finding {} is absent from the later capture, so an entity that could \
                         previously have been reached no longer can be by that route",
                        finding.id
                    ),
                )
                .with_before(Some(Value::String(finding.id.clone())))
                .at_path(element_path("impact", &finding.id)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests_support::minimal;

    #[test]
    fn comparing_a_snapshot_with_itself_produces_no_changes() {
        let snapshot = minimal("2026-01-01T00:00:00Z");
        let diff = compare(&snapshot, &snapshot, "2026-01-02T00:00:00Z").expect("a diff");
        assert!(diff.comparable);
        assert_eq!(diff.changes, Vec::new());
        assert_eq!(diff.summary.as_ref().expect("a summary").total, 0);
        assert_eq!(diff.comparison_mode, ComparisonMode::Canonical);
        diff.validate().expect("an empty diff is a valid diff");
        assert!(!diff.triggers_impact());
    }

    #[test]
    fn a_capture_at_a_later_time_is_not_a_change() {
        // The volatile-field exclusion working end to end: this is the case that would
        // otherwise alert on every scheduled run.
        let before = minimal("2026-01-01T00:00:00Z");
        let after = minimal("2026-06-01T00:00:00Z");
        let diff = compare(&before, &after, "2026-06-01T00:00:00Z").expect("a diff");
        assert!(diff.comparable);
        assert_eq!(diff.before.content_digest, diff.after.content_digest);
        assert_eq!(diff.changes, Vec::new());
    }

    #[test]
    fn a_truncation_change_is_reported_as_a_change_in_coverage() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        after.truncated = true;
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");
        let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        let entry = diff
            .changes
            .iter()
            .find(|change| change.category == ChangeCategory::TruncationChanged)
            .expect("a truncation change");
        assert_eq!(entry.change_type, ChangeType::Modified);
        assert_eq!(entry.before_value, Some(Value::Bool(false)));
        assert_eq!(entry.after_value, Some(Value::Bool(true)));
        assert!(
            !entry.impact_relevant,
            "a change in coverage is not a change in the contract"
        );
        diff.validate().expect("valid");
    }

    #[test]
    fn snapshots_from_different_networks_are_incomparable_rather_than_equal() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        after.network.passphrase = "Public Global Stellar Network ; September 2015".to_owned();
        after.boundary.network.passphrase = after.network.passphrase.clone();
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");

        let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        assert!(!diff.comparable);
        assert_eq!(
            diff.incomparable_reason,
            Some(IncomparableReason::NetworkMismatch)
        );
        assert_eq!(
            diff.changes,
            Vec::new(),
            "an empty diff is indistinguishable from no change, so no changes may be listed"
        );
        assert_eq!(diff.failures().len(), 1, "only the incomparability itself");
        assert!(
            diff.failures()[0]
                .to_string()
                .contains("not comparable because NETWORK_MISMATCH"),
            "got: {}",
            diff.failures()[0]
        );
    }

    #[test]
    fn snapshots_in_the_wrong_ledger_order_are_incomparable() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        after.ledger_boundary = LedgerSequence::new(500).expect("a real ledger");
        after.boundary.ledger = after.ledger_boundary;
        after.id = crate::capture::snapshot_id(&after.contract, &after.boundary);
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");

        let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        assert!(!diff.comparable);
        assert_eq!(
            diff.incomparable_reason,
            Some(IncomparableReason::BoundaryOrderInvalid)
        );
    }

    #[test]
    fn snapshots_from_different_specification_versions_are_incomparable() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        after.spec_version = "2.0.0".to_owned();
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");
        let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        assert_eq!(
            diff.incomparable_reason,
            Some(IncomparableReason::SpecVersionIncompatible)
        );
    }

    #[test]
    fn an_evidence_change_is_reported_with_its_identifier_and_reason() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        let mut extra = crate::capture::tests_support::observation();
        extra.id = "e-2".to_owned();
        after.evidence.push(extra);
        canonicalise(&mut after);
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");

        let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        let added = diff
            .changes
            .iter()
            .find(|change| change.category == ChangeCategory::EvidenceChanged)
            .expect("an evidence change");
        assert_eq!(added.change_type, ChangeType::Added);
        assert!(added.reason.contains("e-2"), "got: {}", added.reason);
        // The path names the record, not the collection: the path is part of the change
        // identifier, and two records added at one boundary are two changes.
        assert_eq!(added.path.as_deref(), Some("/evidence/e-2"));
        diff.validate().expect("valid");
        assert_eq!(
            diff.summary.as_ref().expect("a summary").total,
            diff.changes.len()
        );
    }

    #[test]
    fn a_change_of_confidence_without_a_change_of_relationships_is_reported() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        after.confidence.level = amasario_core::ConfidenceLevel::MediumConfidence;
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");
        let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        let entry = diff
            .changes
            .iter()
            .find(|change| change.category == ChangeCategory::ConfidenceChanged)
            .expect("a confidence change");
        assert!(
            !entry.impact_relevant,
            "the graph did not change, so re-analysis would propagate over the same edges"
        );
    }

    #[test]
    fn a_diff_entry_without_a_reason_is_refused_by_the_diff_that_carries_it() {
        let mut diff = compare(
            &minimal("2026-01-01T00:00:00Z"),
            &minimal("2026-01-01T00:00:00Z"),
            "2026-01-02T00:00:00Z",
        )
        .expect("a diff");
        diff.changes.push(DiffEntry::new(
            ChangeCategory::EvidenceChanged,
            ChangeType::Added,
            EntityRef::new(EntityKind::Contract, "C-a").expect("a reference"),
            "short",
        ));
        diff.summary = Some(DiffSummary::of(&diff.changes));
        let failure = diff
            .failures()
            .into_iter()
            .find(|failure| matches!(failure, SnapshotFailure::ReasonNotStated { .. }))
            .expect("a two-word label is not a reason");
        assert!(failure.to_string().contains("states no reason"));
    }

    #[test]
    fn two_entries_at_one_path_do_not_share_an_identifier() {
        let entity = EntityRef::new(EntityKind::Contract, "C-a").expect("a reference");
        let bare = DiffEntry::new(
            ChangeCategory::EvidenceChanged,
            ChangeType::Added,
            entity,
            "an evidence record appeared at this boundary",
        );
        let pathed = bare.clone().at_path("/evidence");
        assert_ne!(
            bare.id, pathed.id,
            "two differences of one category against one entity at different paths are two \
             differences, and a shared identifier would be reported as a collision"
        );
        assert!(pathed.id.ends_with("/evidence"));
    }

    #[test]
    fn a_summary_that_disagrees_with_its_changes_is_refused() {
        let mut diff = compare(
            &minimal("2026-01-01T00:00:00Z"),
            &minimal("2026-01-01T00:00:00Z"),
            "2026-01-02T00:00:00Z",
        )
        .expect("a diff");
        diff.summary = Some(DiffSummary {
            total: 3,
            by_category: Vec::new(),
            by_change_type: Vec::new(),
        });
        assert!(diff.failures().iter().any(|failure| matches!(
            failure,
            SnapshotFailure::SummaryDisagrees {
                claimed: 3,
                actual: 0
            }
        )));
    }

    #[test]
    fn a_diff_produced_in_another_mode_is_refused_as_a_result() {
        let mut diff = compare(
            &minimal("2026-01-01T00:00:00Z"),
            &minimal("2026-01-01T00:00:00Z"),
            "2026-01-02T00:00:00Z",
        )
        .expect("a diff");
        diff.comparison_mode = ComparisonMode::OrderSensitive;
        assert!(diff.failures().iter().any(|failure| matches!(
            failure,
            SnapshotFailure::ComparisonModeNotCanonical { mode } if mode == "ORDER_SENSITIVE"
        )));
        assert!(!ComparisonMode::OrderSensitive.is_deterministic());
        assert!(ComparisonMode::Canonical.is_deterministic());
    }

    #[test]
    fn two_runs_of_one_comparison_produce_the_same_document() {
        let before = minimal("2026-01-01T00:00:00Z");
        let mut after = minimal("2026-01-01T00:00:00Z");
        after.truncated = true;
        after.confidence.level = amasario_core::ConfidenceLevel::MediumConfidence;
        after.content_digest = crate::normalize::content_digest(&after).expect("digests");

        let first = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        let second = compare(&before, &after, "2026-01-02T00:00:00Z").expect("a diff");
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&second).expect("serialises")
        );
    }

    #[test]
    fn every_category_is_reachable_from_the_vocabulary() {
        // A category with no producer would be a term a consumer subscribes to and never
        // receives, so the ranks and the relevance flags are checked as a set.
        let categories = [
            ChangeCategory::RelationshipAdded,
            ChangeCategory::RelationshipRemoved,
            ChangeCategory::RelationshipChanged,
            ChangeCategory::ProvenanceChanged,
            ChangeCategory::WasmIdentityChanged,
            ChangeCategory::DeploymentChanged,
            ChangeCategory::EvidenceChanged,
            ChangeCategory::ConfidenceChanged,
            ChangeCategory::ImpactSurfaceChanged,
            ChangeCategory::ContractIdentityChanged,
            ChangeCategory::TruncationChanged,
        ];
        let mut ranks: Vec<u8> = categories.iter().map(|category| category.rank()).collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(ranks.len(), categories.len(), "ranks are a total order");
        assert!(
            categories
                .iter()
                .any(|category| category.is_impact_relevant())
        );
        assert!(
            categories
                .iter()
                .any(|category| !category.is_impact_relevant())
        );
    }
}
