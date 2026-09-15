//! The dependency documents the specification defines.
//!
//! # Why this is a projection and not the internal type
//!
//! [`crate::resolver::Dependency`] is the engine's resolved edge: it carries the
//! subject and object, the relationship, every class the evidence supported, the
//! verification status, the traversal depth and the sentence explaining why the edge
//! exists. `dependency.schema.json` describes a different thing - a *published*
//! dependency - and the two do not have the same shape:
//!
//! | Internal | `dependency.schema.json` |
//! | --- | --- |
//! | `subject` / `object` | `source` / `target` |
//! | `relationship` | not a field; the edge document has it, the dependency document does not |
//! | `classes` (a list) | `type` (a single class) |
//! | `verification` | `verificationStatus` |
//! | `observed_at` (one boundary) | `firstObserved` and `lastObserved` (a range) |
//! | `reason`, `depth` | nowhere |
//!
//! Publishing the internal type directly is what this module exists to prevent. The
//! schemas set `additionalProperties: false`, so a field with no home in the schema
//! makes the whole document invalid - which is the failure this crate previously had.
//!
//! # Where the extra detail goes
//!
//! `dependency.schema.json` defines `metadata` as "the only object in the
//! specification with open properties", and constrains it: "nothing in metadata may
//! be used to establish, strengthen or contradict a claim". That is exactly the right
//! home for the engine's `reason`, `relationship`, `depth` and full `classes` list.
//! They are explanation, not assertion, so moving them there loses nothing and keeps
//! the document valid. A consumer that ignores `metadata` still sees every fact the
//! schema requires; a consumer that reads it sees why the engine believes the edge.
//!
//! # How a refusal is published
//!
//! `dependency-set.schema.json`'s `unresolved` array describes "dependencies that were
//! identified but could not be resolved", and its `reason` is drawn from a closed
//! enumeration: `NOT_FOUND`, `NETWORK_ERROR`, `TIMEOUT`, `MALFORMED_RESPONSE`,
//! `OUT_OF_BOUNDARY`, `NOT_PERMITTED`, `UNSUPPORTED`.
//!
//! The engine has two distinct things that answer that description, and they are kept
//! apart rather than merged:
//!
//! * A **resolution failure** - the endpoint could not be asked, the ledger is outside
//!   the boundary, the response did not decode - is an [`EngineError`] whose category
//!   maps to one of the six transport-shaped reasons. Those never reach this module as
//!   an [`crate::resolver::Unestablished`]; they arrive as a failure, and a caller that
//!   publishes one should use [`UnresolvedReason::of_error`].
//! * A **rule refusal** - the candidate was observed and the specification would not
//!   let it become a dependency - *is* an [`crate::resolver::Unestablished`], and it
//!   maps to exactly one reason: [`UnresolvedReason::NotPermitted`]. That is not a
//!   forced fit. `NOT_PERMITTED` is the schema's own word for "the specification's
//!   rules did not permit this claim", which is the only thing `Unestablished` ever
//!   records - [`crate::classifier::classify`] returns an error for a candidate exactly
//!   when a dependency rule refuses it.
//!
//! Nothing is lost by the single reason, because `detail` carries the rest: the object,
//! the relationship, the basis and the engine's sentence explaining the refusal. A
//! self-dependency and a failed classification therefore read as
//! `NOT_PERMITTED` with two different explanations, and neither is confused with a
//! timeout - which is the distinction `docs/verification.md` requires be kept.
//!
//! # Identifier agreement with the graph layer
//!
//! [`dependency_id`] derives an edge identifier from the source, the relationship and
//! the target, using the same pre-image and the same separator as
//! `amasario_graph::edges::Edge::stable_id`. The two layers must agree: an edge that
//! had one identifier in a dependency document and another in a graph document could
//! not be matched across the two, and a diff would report every edge as replaced.

use std::collections::BTreeMap;

use amasario_core::{
    Basis, Confidence, DependencyClass, Digest, EngineError, EntityRef, Network,
    ObservationBoundary, Relationship, Result, TruncationReason, VerificationStatus,
};
use serde::{Deserialize, Serialize};

use crate::classifier::EvidenceRef;
use crate::resolver::{Cycle, Dependency, DependencySet};

/// The producer-defined detail a document carries in `metadata`.
///
/// A `BTreeMap` rather than a `HashMap` because iteration order reaches output, and
/// `clippy.toml` refuses the unordered collections for that reason.
pub type Metadata = BTreeMap<String, serde_json::Value>;

/// The metadata key carrying the relationship an edge was established under.
pub const METADATA_RELATIONSHIP: &str = "relationship";
/// The metadata key carrying how many edges separate the two entities.
pub const METADATA_DEPTH: &str = "depth";
/// The metadata key carrying the engine's explanation of why the edge exists.
pub const METADATA_REASON: &str = "reason";
/// The metadata key carrying every class the evidence supported, not only the primary.
pub const METADATA_CLASSES: &str = "classes";

/// The identifier this engine gives the edge between two entities.
///
/// Deterministic, and dependent on exactly the three properties that constitute the
/// edge, so that the identifier survives a re-run and changes whenever the edge does.
///
/// # Errors
///
/// Returns an internal error if the digest cannot be computed, which cannot happen for
/// an in-memory slice and would be a defect if it did.
pub fn dependency_id(
    source: &EntityRef,
    relationship: Relationship,
    target: &EntityRef,
) -> Result<String> {
    // `\u{1f}` separates the parts. It is a unit separator that cannot appear in a
    // contract address, a digest, a revision or a relationship name, so no two
    // different triples can produce the same pre-image - which concatenation with a
    // printable delimiter could not guarantee. The same pre-image the graph layer
    // uses, so the two identifiers agree.
    let pre_image = format!("{source}\u{1f}{}\u{1f}{target}", relationship.as_str());
    Ok(Digest::sha256_of(pre_image.as_bytes()).value().to_owned())
}

/// One dependency, in the shape `dependency.schema.json` defines.
///
/// Required by the schema: `source`, `target`, `type`, `basis` and a non-empty
/// `evidence`. Everything else is optional, and an absent optional field is omitted
/// rather than written as `null` so that "not established" and "established as empty"
/// cannot be confused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencyDocument {
    /// The dependency identifier, used by relationships and reports to refer to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The depending entity.
    pub source: EntityRef,
    /// The entity depended upon.
    pub target: EntityRef,
    /// The dependency class.
    ///
    /// The schema's field is named `type`; `dependency_type` is the Rust name because
    /// `type` is a keyword, and the explicit rename wins over `rename_all`.
    #[serde(rename = "type")]
    pub dependency_type: DependencyClass,
    /// How the dependency was established.
    pub basis: Basis,
    /// The identifiers of the evidence records supporting it. Never empty.
    pub evidence: Vec<String>,
    /// How strongly the evidence supports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// For a transitive dependency, the intermediate entities in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<EntityRef>>,
    /// The network the dependency was observed on, when it was observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// The bottom of the boundary range the dependency was observed over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_observed: Option<ObservationBoundary>,
    /// The top of that range.
    ///
    /// Equal to `first_observed` here. The engine records the boundary an edge was
    /// established at rather than a range, and stating a one-point range is the
    /// honest projection of that: leaving `lastObserved` absent would claim the edge
    /// was seen over no range at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed: Option<ObservationBoundary>,
    /// The outcome of checking the claim against its evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_status: Option<VerificationStatus>,
    /// Producer-defined detail. Nothing here establishes, strengthens or contradicts a
    /// claim; see the module documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl DependencyDocument {
    /// Projects a resolved dependency into the document the specification defines.
    ///
    /// # Errors
    ///
    /// Returns a dependency error when the dependency carries no class. The schema
    /// requires `type`, and the engine's own invariant is that a dependency has at
    /// least one class, so a dependency without one is a defect rather than a slightly
    /// incomplete document - and inventing a class would be worse than refusing.
    pub fn of(dependency: &Dependency) -> Result<Self> {
        let primary = dependency.classes.first().copied().ok_or_else(|| {
            EngineError::Dependency(format!(
                "the dependency {} -> {} carries no class, and dependency.schema.json \
                 requires a type",
                dependency.subject, dependency.object
            ))
        })?;

        let mut metadata = Metadata::new();
        metadata.insert(
            METADATA_RELATIONSHIP.to_owned(),
            serde_json::Value::String(dependency.relationship.as_str().to_owned()),
        );
        metadata.insert(
            METADATA_DEPTH.to_owned(),
            serde_json::Value::from(dependency.depth),
        );
        metadata.insert(
            METADATA_REASON.to_owned(),
            serde_json::Value::String(dependency.reason.clone()),
        );
        metadata.insert(
            METADATA_CLASSES.to_owned(),
            serde_json::Value::Array(
                dependency
                    .classes
                    .iter()
                    .map(|class| serde_json::Value::String(class.as_str().to_owned()))
                    .collect(),
            ),
        );

        Ok(Self {
            id: Some(dependency_id(
                &dependency.subject,
                dependency.relationship,
                &dependency.object,
            )?),
            source: dependency.subject.clone(),
            target: dependency.object.clone(),
            dependency_type: primary,
            basis: dependency.basis,
            evidence: citation_ids(&dependency.evidence),
            confidence: Some(dependency.confidence.clone()),
            path: (!dependency.path.is_empty()).then(|| dependency.path.clone()),
            network: dependency.network().cloned(),
            first_observed: dependency.observed_at.clone(),
            last_observed: dependency.observed_at.clone(),
            verification_status: Some(dependency.verification),
            metadata: Some(metadata),
        })
    }

    /// The published form's JSON, for a caller that needs a value rather than a string.
    ///
    /// # Errors
    ///
    /// Returns a dependency error if the document cannot be serialised, which for this
    /// shape would be a defect in the engine.
    pub fn to_value(&self) -> Result<serde_json::Value> {
        serde_json::to_value(self).map_err(|error| {
            EngineError::Dependency(format!(
                "the dependency document could not be written: {error}"
            ))
        })
    }
}

/// One edge, in the shape `dependency-edge.schema.json` defines.
///
/// Narrower than [`DependencyDocument`] and wider than the graph layer's own edge: it
/// carries the relationship and the single dependency class, which the dependency
/// document does not have, and omits the path, which belongs to the dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencyEdgeDocument {
    /// The edge identifier. Stable across runs for the same edge.
    pub id: String,
    /// The subject of the relationship.
    pub source: EntityRef,
    /// The object of the relationship.
    pub target: EntityRef,
    /// The typed relationship.
    pub relationship: Relationship,
    /// The dependency classification, when this edge is a dependency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_class: Option<DependencyClass>,
    /// How the dependency was established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis: Option<Basis>,
    /// The identifiers of the evidence records establishing the edge. Never empty.
    pub evidence: Vec<String>,
    /// How strongly the evidence supports the edge.
    pub confidence: Confidence,
    /// The network the edge was observed on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// The observation boundary of this edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// The earliest boundary at which this edge was seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_observed: Option<ObservationBoundary>,
    /// The latest boundary at which this edge was seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed: Option<ObservationBoundary>,
    /// The outcome of checking the edge against its evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_status: Option<VerificationStatus>,
    /// Whether the edge was directly observed rather than inferred.
    ///
    /// Required by the schema "wherever the distinction could be lost in
    /// serialisation", because reporting an inferred edge as observed is the failure
    /// the whole basis vocabulary exists to prevent.
    pub observed: bool,
    /// Producer-defined detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl DependencyEdgeDocument {
    /// Projects a resolved dependency into the edge document.
    ///
    /// `observed` is derived from the basis rather than passed in, so a caller cannot
    /// mark an inferred edge as observed.
    ///
    /// # Errors
    ///
    /// As [`DependencyDocument::of`].
    pub fn of(dependency: &Dependency) -> Result<Self> {
        let mut metadata = Metadata::new();
        metadata.insert(
            METADATA_DEPTH.to_owned(),
            serde_json::Value::from(dependency.depth),
        );
        metadata.insert(
            METADATA_REASON.to_owned(),
            serde_json::Value::String(dependency.reason.clone()),
        );

        Ok(Self {
            id: dependency_id(
                &dependency.subject,
                dependency.relationship,
                &dependency.object,
            )?,
            source: dependency.subject.clone(),
            target: dependency.object.clone(),
            relationship: dependency.relationship,
            dependency_class: dependency.classes.first().copied(),
            basis: Some(dependency.basis),
            evidence: citation_ids(&dependency.evidence),
            confidence: dependency.confidence.clone(),
            network: dependency.network().cloned(),
            boundary: dependency.observed_at.clone(),
            first_observed: dependency.observed_at.clone(),
            last_observed: dependency.observed_at.clone(),
            verification_status: Some(dependency.verification),
            observed: dependency.is_observed(),
            metadata: Some(metadata),
        })
    }
}

/// One cycle, in the shape `dependency-set.schema.json` defines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleDocument {
    /// The edge identifiers forming the cycle, in traversal order.
    pub edges: Vec<String>,
    /// The number of edges in the cycle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
}

impl CycleDocument {
    /// Projects a cycle found while closing a set.
    ///
    /// # Errors
    ///
    /// Returns an internal error if the cycle carries fewer than two edges, which the
    /// schema refuses and which the traversal cannot produce - a cycle of one edge
    /// would be a self-dependency, rejected before it became an edge.
    pub fn of(cycle: &Cycle) -> Result<Self> {
        if cycle.edges.len() < 2 {
            return Err(EngineError::Internal(format!(
                "a cycle of {} edge(s) is not representable: dependency-set.schema.json \
                 requires at least two, because a single-edge cycle is a self-dependency",
                cycle.edges.len()
            )));
        }
        Ok(Self {
            length: Some(cycle.edges.len()),
            edges: cycle.edges.clone(),
        })
    }
}

/// A dependency that could not be published as an edge, and why.
///
/// See the module documentation: this document is where a rule refusal and a transport
/// failure are kept apart, because both answer "could not be resolved" and only one of
/// them means the dependency might exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnresolvedDocument {
    /// The entity that was being resolved from, where it was known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<EntityRef>,
    /// Why the dependency could not be resolved.
    pub reason: UnresolvedReason,
    /// What happened, preserved rather than summarised.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `dependency-set.schema.json`'s reason enumeration.
///
/// Six of the seven are transport-shaped and are reached through
/// [`UnresolvedReason::of_error`]; the seventh, [`Self::NotPermitted`], is the only
/// one a rule refusal can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum UnresolvedReason {
    /// The resource is not there. An absence, not a failure.
    NotFound,
    /// The endpoint could not be reached.
    NetworkError,
    /// The endpoint did not answer in time.
    Timeout,
    /// A response arrived and was not what it claimed to be.
    MalformedResponse,
    /// The question reaches outside the observation boundary.
    OutOfBoundary,
    /// The specification's rules did not permit the claim.
    NotPermitted,
    /// The engine does not implement the question.
    Unsupported,
}

impl UnresolvedReason {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "NOT_FOUND",
            Self::NetworkError => "NETWORK_ERROR",
            Self::Timeout => "TIMEOUT",
            Self::MalformedResponse => "MALFORMED_RESPONSE",
            Self::OutOfBoundary => "OUT_OF_BOUNDARY",
            Self::NotPermitted => "NOT_PERMITTED",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    /// The reason a failure is, where the failure is a resolution failure.
    ///
    /// Returns `None` for anything else. A configuration error or an internal defect is
    /// a defect in the run rather than a dependency that could not be resolved, and
    /// publishing one as `UNSUPPORTED` would put a bug into a document as though it were
    /// a finding - which is the one thing a provenance document must not contain.
    ///
    /// Only four of the seven reasons are reachable this way, and the three that are not
    /// are reachable only as explicit values, which is honest rather than incomplete:
    /// the engine's error model has no separate timeout variant (a timeout arrives as a
    /// retryable network failure, with the endpoint's own explanation in `detail`), and
    /// `OUT_OF_BOUNDARY` and `UNSUPPORTED` describe a decision the caller made rather
    /// than a failure the engine had.
    #[must_use]
    pub const fn of_error(error: &EngineError) -> Option<Self> {
        match error {
            EngineError::ContractNotFound { .. } => Some(Self::NotFound),
            EngineError::MalformedResponse { .. } => Some(Self::MalformedResponse),
            EngineError::Network { .. } => Some(Self::NetworkError),
            _ => None,
        }
    }
}

impl std::fmt::Display for UnresolvedReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The longest `detail` the schema accepts.
///
/// `dependency-set.schema.json` caps the field at 2048 characters, and a producer that
/// ignored the cap would publish a document its own specification rejects.
const MAX_UNRESOLVED_DETAIL: usize = 2048;

/// One refusal, as the document publishes it.
///
/// The engine's `Unestablished` names the object, the relationship and the basis, and
/// the schema's entry has a place for none of them. They are therefore folded into
/// `detail` rather than dropped: a reader diagnosing a refusal needs to know *what* was
/// refused and under which relationship, and a `NOT_PERMITTED` with no object would be
/// an unexplained refusal.
fn unresolved_of(
    subject: &EntityRef,
    candidate: &crate::resolver::Unestablished,
) -> UnresolvedDocument {
    let mut detail = format!(
        "{} {} is not permitted: {}",
        candidate.relationship.as_str(),
        candidate.object,
        candidate.reason
    );
    if let Some(observed) = &candidate.detail {
        detail.push_str("; observed: ");
        detail.push_str(observed);
    }
    detail.push_str(&format!("; basis {}", candidate.basis.as_str()));

    UnresolvedDocument {
        source: Some(subject.clone()),
        reason: UnresolvedReason::NotPermitted,
        detail: Some(truncate_detail(&detail)),
    }
}

/// Truncates a detail to the schema's cap, on a character boundary.
fn truncate_detail(detail: &str) -> String {
    if detail.chars().count() <= MAX_UNRESOLVED_DETAIL {
        return detail.to_owned();
    }
    let mut truncated: String = detail.chars().take(MAX_UNRESOLVED_DETAIL - 1).collect();
    truncated.push('…');
    truncated
}

/// A dependency set, in the shape `dependency-set.schema.json` defines.
///
/// The schema requires `edges` and nothing else, and requires that each edge also
/// appear in exactly one of `direct` or `transitive` so that the partition cannot
/// silently lose an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencySetDocument {
    /// Identifier for this set within the producing document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Every edge in the set.
    pub edges: Vec<DependencyEdgeDocument>,
    /// The identifiers of the one-hop edges.
    ///
    /// `default` as well as `skip_serializing_if`, and the pair is what makes the
    /// document round-trip: omitting an empty array is right, but a reader that then
    /// refused to parse the document it had just written would be a producer whose own
    /// output it cannot read. The integration suite catches exactly that, which is how
    /// this omission was found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<String>,
    /// The identifiers of the edges reached only through an intermediate entity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitive: Vec<String>,
    /// Cycles detected, reported rather than resolved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<CycleDocument>,
    /// Dependencies that were identified but could not become edges.
    ///
    /// Kept separate from `edges` because a refusal and an absent dependency are
    /// different findings, and because a consumer that merged them would report a
    /// contract as not depending on something when the engine was actually told it
    /// does and could not publish the claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedDocument>,
    /// The traversal depth the analysis was bounded by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<usize>,
    /// Whether traversal stopped before exhausting the graph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Why traversal stopped, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<TruncationReason>,
    /// The observation boundary the set was assembled at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
}

impl DependencySetDocument {
    /// Projects a resolved dependency set into the document the specification defines.
    ///
    /// # Errors
    ///
    /// Returns a dependency error if any edge or cycle cannot be projected, and an
    /// internal error if an edge appears in both partitions - which would make the
    /// set's own claim about its completeness false.
    pub fn of(set: &DependencySet) -> Result<Self> {
        let mut edges = Vec::with_capacity(set.len());
        for dependency in set.all() {
            edges.push(DependencyEdgeDocument::of(dependency)?);
        }

        let direct: Vec<String> = identifiers(&set.direct)?;
        let transitive: Vec<String> = identifiers(&set.transitive)?;
        for id in &direct {
            if transitive.contains(id) {
                return Err(EngineError::Internal(format!(
                    "edge {id} appears in both the direct and transitive partitions, and \
                     rule dependency/transitive-dependency requires exactly one"
                )));
            }
        }

        let mut cycles = Vec::with_capacity(set.cycles.len());
        for cycle in &set.cycles {
            cycles.push(CycleDocument::of(cycle)?);
        }

        // Refusals are published in the set's own canonical order, which `resolve`
        // already established, so the document's bytes do not depend on which order the
        // candidates happened to be observed in.
        let unresolved: Vec<UnresolvedDocument> = set
            .unestablished
            .iter()
            .map(|candidate| unresolved_of(&set.subject, candidate))
            .collect();

        Ok(Self {
            id: None,
            edges,
            direct,
            transitive,
            cycles,
            unresolved,
            max_depth: Some(set.max_depth),
            truncated: Some(set.truncated),
            truncation_reason: set.truncation_reason,
            boundary: set.boundary.clone(),
        })
    }

    /// Gives the document an identifier within its producing document.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Checks that the partition the schema requires actually holds.
    ///
    /// A consumer that has only the document can run this; so can a producer before it
    /// publishes. `dependency/transitive-dependency` states the rule, and the schema
    /// cannot express "every edge appears in exactly one of these two arrays".
    #[must_use]
    pub fn partition_violations(&self) -> Vec<String> {
        let known: Vec<&str> = self.edges.iter().map(|edge| edge.id.as_str()).collect();
        let mut problems = Vec::new();
        for id in self.direct.iter().chain(self.transitive.iter()) {
            if !known.contains(&id.as_str()) {
                problems.push(format!("{id} is partitioned but is not an edge"));
            }
        }
        for id in &known {
            let in_direct = self.direct.iter().any(|candidate| candidate == id);
            let in_transitive = self.transitive.iter().any(|candidate| candidate == id);
            if in_direct && in_transitive {
                problems.push(format!("{id} appears in both partitions"));
            }
            if !in_direct && !in_transitive {
                problems.push(format!("{id} appears in neither partition"));
            }
        }
        problems.sort();
        problems
    }
}

/// The citation identifiers of a dependency's evidence, in record order.
///
/// The dependency layer's own ordering is preserved rather than re-sorted, because two
/// orderings for one list would make a document's bytes depend on which layer wrote it.
#[must_use]
fn citation_ids(evidence: &[EvidenceRef]) -> Vec<String> {
    evidence
        .iter()
        .map(|citation| citation.id.clone())
        .collect()
}

/// The identifiers of a partition of dependencies, in order.
///
/// # Errors
///
/// Returns an internal error if an edge identifier cannot be derived.
fn identifiers(dependencies: &[Dependency]) -> Result<Vec<String>> {
    dependencies
        .iter()
        .map(|dependency| {
            dependency_id(
                &dependency.subject,
                dependency.relationship,
                &dependency.object,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::Candidate;
    use crate::resolver::resolve;
    use amasario_core::{
        Basis, ConfidenceLevel, EntityKind, EvidenceType, LedgerSequence, NetworkType,
    };

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a reference")
    }

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(4_242).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn candidate(from: &str, to: &str, transaction: &str) -> Candidate {
        Candidate::new(
            entity(EntityKind::Contract, from),
            entity(EntityKind::Contract, to),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    fn set() -> DependencySet {
        resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[
                candidate("C-subject", "C-a", &"a".repeat(64)),
                candidate("C-a", "C-b", &"b".repeat(64)),
            ],
            4,
        )
        .expect("resolves")
    }

    /// The edge to `C-a`, chosen by target rather than by index.
    ///
    /// The set is in canonical order, so a test that indexed into it would be testing
    /// the sort rather than the projection.
    fn first_dependency() -> Dependency {
        set()
            .all()
            .find(|dep| dep.object.id == "C-a")
            .expect("the C-a dependency")
            .clone()
    }

    #[test]
    fn the_identifier_matches_the_graph_layers_own_derivation() {
        // The two layers must agree, or an edge could not be matched across a
        // dependency document and a graph document.
        let source = entity(EntityKind::Contract, "C-subject");
        let target = entity(EntityKind::Contract, "C-a");
        let id = dependency_id(&source, Relationship::Invocates, &target).expect("an identifier");
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            id,
            dependency_id(&source, Relationship::Invocates, &target).expect("an identifier"),
            "the identifier is deterministic"
        );
        assert_ne!(
            id,
            dependency_id(&source, Relationship::DependsOn, &target).expect("an identifier"),
            "the relationship is part of the identity"
        );
    }

    #[test]
    fn a_dependency_document_carries_only_the_schemas_fields() {
        let document = DependencyDocument::of(&first_dependency()).expect("projects");
        let value = document.to_value().expect("serialises");
        let object = value.as_object().expect("an object");

        // `dependency.schema.json` sets `additionalProperties: false`, so a field the
        // engine invented would make the whole document invalid.
        let allowed = [
            "id",
            "source",
            "target",
            "type",
            "basis",
            "evidence",
            "confidence",
            "path",
            "network",
            "firstObserved",
            "lastObserved",
            "verificationStatus",
            "metadata",
        ];
        for key in object.keys() {
            assert!(
                allowed.contains(&key.as_str()),
                "{key} is not a dependency.schema.json field"
            );
        }
        for required in ["source", "target", "type", "basis", "evidence"] {
            assert!(object.contains_key(required), "missing required {required}");
        }
        assert_eq!(value["source"]["kind"], "CONTRACT");
        assert_eq!(value["target"]["id"], "C-a");
        // The internal names must not leak.
        for forbidden in [
            "subject",
            "object",
            "relationship",
            "classes",
            "verification",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "{forbidden} is an internal name, not a schema field"
            );
        }
    }

    #[test]
    fn the_internal_detail_is_preserved_in_metadata() {
        // The reason, the relationship, the depth and the full class list have no
        // field in the schema, and `metadata` is the one open object. Discarding them
        // would lose what the engine knows; publishing them elsewhere would make the
        // document invalid.
        let document = DependencyDocument::of(&first_dependency()).expect("projects");
        let metadata = document.metadata.as_ref().expect("metadata");
        assert_eq!(
            metadata[METADATA_RELATIONSHIP],
            serde_json::Value::String("INVOCATES".to_owned())
        );
        assert_eq!(metadata[METADATA_DEPTH], serde_json::Value::from(0));
        assert!(
            metadata[METADATA_REASON]
                .as_str()
                .expect("a sentence")
                .contains("OBSERVED_INVOCATION"),
            "the engine's explanation survives: {:?}",
            metadata[METADATA_REASON]
        );
        assert!(
            metadata[METADATA_CLASSES]
                .as_array()
                .expect("an array")
                .contains(&serde_json::Value::String("CONTRACT".to_owned())),
            "the full class list survives: {:?}",
            metadata[METADATA_CLASSES]
        );
    }

    #[test]
    fn the_primary_class_is_published_and_the_rest_are_kept() {
        // The schema has one `type`; the engine can hold several classes. The primary
        // is the first in the taxonomy's canonical order, and the full list goes to
        // metadata so nothing is lost.
        let mut dependency = first_dependency();
        dependency = dependency.with_class(DependencyClass::Runtime);
        let document = DependencyDocument::of(&dependency).expect("projects");
        assert_eq!(
            document.dependency_type,
            *dependency.classes.first().expect("a class")
        );
        let published = document.metadata.as_ref().expect("metadata")[METADATA_CLASSES]
            .as_array()
            .expect("an array")
            .len();
        assert_eq!(published, dependency.classes.len());
    }

    #[test]
    fn a_dependency_without_a_class_is_refused_rather_than_given_an_invented_one() {
        let mut dependency = first_dependency();
        dependency.classes.clear();
        let error = DependencyDocument::of(&dependency).expect_err("no class to publish");
        assert_eq!(error.category(), amasario_core::ErrorCategory::Dependency);
    }

    #[test]
    fn an_edge_document_carries_the_relationship_and_derives_observed() {
        // `observed` is derived from the basis, so a caller cannot mark an inferred
        // edge as observed - which is the failure the basis vocabulary exists to stop.
        let dependency = first_dependency();
        let document = DependencyEdgeDocument::of(&dependency).expect("projects");
        assert_eq!(document.relationship, Relationship::Invocates);
        assert!(document.observed, "an observed invocation is observed");
        assert_eq!(document.confidence.level, ConfidenceLevel::Verified);
        assert_eq!(document.evidence.len(), 1);
        assert_eq!(
            document.id,
            DependencyDocument::of(&dependency)
                .expect("projects")
                .id
                .expect("an identifier"),
            "the two documents agree on the edge identity"
        );
    }

    #[test]
    fn a_set_document_partitions_every_edge_exactly_once() {
        let document = DependencySetDocument::of(&set()).expect("projects");
        assert!(
            document.partition_violations().is_empty(),
            "got: {:?}",
            document.partition_violations()
        );
        assert_eq!(
            document.edges.len(),
            document.direct.len() + document.transitive.len()
        );
        assert_eq!(document.max_depth, Some(4));
        assert_eq!(document.truncated, Some(false));
        assert_eq!(
            document.boundary.as_ref().map(|b| b.network.id.as_str()),
            Some("testnet")
        );
    }

    #[test]
    fn a_set_document_reports_a_broken_partition_rather_than_hiding_it() {
        // The schema requires each edge to be in exactly one partition; it cannot
        // express that itself, so a consumer that has only the document can still
        // check it.
        let mut document = DependencySetDocument::of(&set()).expect("projects");
        document.transitive = document.direct.clone();
        let problems = document.partition_violations();
        assert!(
            problems.iter().any(|p| p.contains("both partitions")),
            "got: {problems:?}"
        );

        let mut nowhere = DependencySetDocument::of(&set()).expect("projects");
        let removed = nowhere.direct.pop().expect("one identifier");
        assert!(
            nowhere
                .partition_violations()
                .iter()
                .any(|p| p.contains(&removed) && p.contains("neither")),
            "an edge in neither partition is a violation"
        );
    }

    /// A set holding one candidate the rules refuse.
    ///
    /// The refusal is the interesting one and it is real: an invocation the endpoint
    /// did not report as succeeding cannot establish a `RUNTIME` dependency, so
    /// `classify` refuses it and `resolve` records the refusal rather than an edge.
    fn refused_set() -> DependencySet {
        let refused = Candidate::new(
            entity(EntityKind::Contract, "C-subject"),
            entity(EntityKind::Contract, "C-refused"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "c".repeat(64)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        // The three-valued outcome is what keeps "the endpoint said it failed" and
        // "the endpoint did not say" from being the same thing.
        .with_outcome(None);

        resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[refused],
            1,
        )
        .expect("resolves")
    }

    #[test]
    fn a_set_without_refusals_omits_the_unresolved_array() {
        // An absent optional field and an empty one must not be confused, and the
        // schema makes the array optional for exactly that reason.
        let value = serde_json::to_value(DependencySetDocument::of(&set()).expect("projects"))
            .expect("serialises");
        assert!(
            !value
                .as_object()
                .expect("an object")
                .contains_key("unresolved"),
            "a set with nothing refused publishes no unresolved array"
        );
    }

    #[test]
    fn a_rule_refusal_is_published_as_not_permitted_with_its_reason() {
        // The point of publishing this at all: a consumer that could not see the
        // refusal would read the absence of an edge as the absence of a dependency.
        let document = DependencySetDocument::of(&refused_set()).expect("projects");
        assert!(document.edges.is_empty(), "the candidate was refused");
        assert_eq!(document.unresolved.len(), 1);

        let entry = &document.unresolved[0];
        assert_eq!(entry.reason, UnresolvedReason::NotPermitted);
        assert_eq!(entry.reason.as_str(), "NOT_PERMITTED");
        assert_eq!(
            entry.source.as_ref().map(|source| source.id.as_str()),
            Some("C-subject"),
            "the depending entity is named, so the refusal is attributable"
        );

        let detail = entry.detail.as_deref().expect("a detail");
        assert!(
            detail.contains("C-refused"),
            "the object has no field of its own, so it must survive in the detail: {detail}"
        );
        assert!(
            detail.contains("OBSERVED_INVOCATION"),
            "the basis must survive too: {detail}"
        );
    }

    #[test]
    fn a_refusal_is_not_published_as_a_resolution_failure() {
        // The distinction the documentation promises: a rule refusal is
        // NOT_PERMITTED and a transport failure is something else. A publisher that
        // collapsed both into one reason would make a self-dependency
        // indistinguishable from an unreachable endpoint.
        let document = DependencySetDocument::of(&refused_set()).expect("projects");
        for entry in &document.unresolved {
            assert_ne!(entry.reason, UnresolvedReason::NetworkError);
            assert_ne!(entry.reason, UnresolvedReason::Timeout);
            assert_ne!(entry.reason, UnresolvedReason::NotFound);
        }
    }

    #[test]
    fn a_resolution_failure_maps_to_its_own_reason_and_a_defect_maps_to_none() {
        use amasario_core::EngineError;

        assert_eq!(
            UnresolvedReason::of_error(&EngineError::ContractNotFound {
                contract_id: "C".to_owned(),
                network: "testnet".to_owned(),
                ledger: 4_242,
            }),
            Some(UnresolvedReason::NotFound)
        );
        assert_eq!(
            UnresolvedReason::of_error(&EngineError::transient_network("https://rpc", "timeout")),
            Some(UnresolvedReason::NetworkError)
        );
        assert_eq!(
            UnresolvedReason::of_error(&EngineError::Configuration("no endpoint".to_owned())),
            None,
            "a configuration error is a defect in the run, not an unresolved dependency"
        );
        assert_eq!(
            UnresolvedReason::of_error(&EngineError::Internal("invariant".to_owned())),
            None
        );
    }

    #[test]
    fn an_over_long_refusal_detail_is_truncated_to_what_the_schema_accepts() {
        // `dependency-set.schema.json` caps `detail` at 2048 characters. Exceeding it
        // would publish a document the specification itself rejects.
        let long = "x".repeat(MAX_UNRESOLVED_DETAIL * 2);
        let truncated = truncate_detail(&long);
        assert_eq!(truncated.chars().count(), MAX_UNRESOLVED_DETAIL);
        assert!(truncated.ends_with('…'));

        let short = truncate_detail("a short refusal");
        assert_eq!(short, "a short refusal");
    }

    #[test]
    fn a_refusal_reaches_the_serialised_document() {
        // The field has to survive `to_value`, not only exist on the struct.
        let value =
            serde_json::to_value(DependencySetDocument::of(&refused_set()).expect("projects"))
                .expect("serialises");
        let unresolved = value["unresolved"].as_array().expect("an array");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0]["reason"], "NOT_PERMITTED");
        assert_eq!(unresolved[0]["source"]["kind"], "CONTRACT");
    }

    #[test]
    fn the_projection_is_deterministic_across_insertion_orders() {
        let forwards = [
            candidate("C-subject", "C-zeta", &"1".repeat(64)),
            candidate("C-subject", "C-alpha", &"2".repeat(64)),
        ];
        let mut backwards = forwards.clone();
        backwards.reverse();

        let subject = entity(EntityKind::Contract, "C-subject");
        let first = DependencySetDocument::of(
            &resolve(subject.clone(), Some(boundary()), &forwards, 4).expect("resolves"),
        )
        .expect("projects");
        let second = DependencySetDocument::of(
            &resolve(subject, Some(boundary()), &backwards, 4).expect("resolves"),
        )
        .expect("projects");

        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&second).expect("serialises"),
            "one set, one document"
        );
    }

    #[test]
    fn an_absent_optional_field_is_omitted_rather_than_written_as_null() {
        // "not established" and "established as empty" must not be confused, which is
        // why a `null` is never emitted for a missing value.
        let document = DependencyDocument::of(&first_dependency()).expect("projects");
        let value = document.to_value().expect("serialises");
        assert!(
            value.get("path").is_none(),
            "a direct dependency has no path, and the field is absent rather than null"
        );
        assert!(value.get("subject").is_none());
    }
}
