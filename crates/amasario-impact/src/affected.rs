//! The impact finding: what was affected, how far away, and on what evidence.
//!
//! # What a finding has to carry to be worth reading
//!
//! `schema/impact.schema.json` is explicit: "An impact finding is deliberately not a
//! score." It carries the changed entity, the affected entity, the path between them,
//! the hop count, the relationship types along that path, the evidence, the confidence,
//! the reason, the change type and the verification state, "so that a reader can
//! disagree with the conclusion by inspecting the reasoning instead of having to trust a
//! number." [`ImpactFinding`] is that record, and [`ImpactFinding::failures`] is the
//! mechanical half of the same promise: every structural claim a finding makes about
//! itself is checkable, so a malformed finding is rejected rather than published.
//!
//! # Classification is derived, not chosen
//!
//! The distance terms are a function of `hopDepth` - depth one is `DIRECT`, depth two
//! or more is `TRANSITIVE`, depth three or more also carries `MULTI_HOP` - and the
//! entity terms are a function of the affected entity's kind. Both derivations are
//! implemented once, in [`required_distance_terms`] and [`required_entity_term`], and
//! enforced by [`ImpactFinding::failures`]. A finding therefore cannot be published as
//! `DIRECT` at depth four, and cannot describe a contract finding without `CONTRACT`.
//!
//! # Which deployments can be affected at all
//!
//! [`DeploymentRecord`] exists because `impact/deployment-impact` excludes
//! `UNCONFIRMED`, `FAILED` and `UNKNOWN` deployments: an impact finding asserts that a
//! change may reach something, and each of those three statuses says the thing has not
//! been established as existing. The exclusion is applied during traversal rather than
//! at validation, so an ineligible deployment is not reported *and* the entities behind
//! it are not reached either - the executable a failed deployment would have installed
//! is not the one at that address.

use amasario_core::{
    Confidence, ConfidenceLevel, Digest, EngineError, EntityKind, EntityRef, LedgerSequence,
    ObservationBoundary, Relationship, Result, VerificationStatus,
};
use amasario_dependency::EvidenceRef;
use amasario_provenance::{DeploymentProvenance, DeploymentStatus};
use serde::{Deserialize, Serialize};

use crate::errors::{ImpactFailure, first_failure};
use crate::propagation::{ImpactDirection, Step, combine_directions};

/// The shortest a reason may be before it stops being one.
///
/// Eight characters, matching the schema's `minLength`. A reason shorter than this is
/// a label rather than a justification, and the field exists to replace a score.
const MINIMUM_REASON_LENGTH: usize = 8;

/// A classification of a single impact finding.
///
/// A set of these, not one: `taxonomies/impact-types.yaml` notes that "a change
/// reaching a contract three hops away is simultaneously `TRANSITIVE`, `MULTI_HOP` and
/// `CONTRACT`". The three families - distance, entity and trigger - are kept apart in
/// code because each is derived from a different fact and only the distance family is
/// mechanically checkable from the finding alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImpactType {
    /// The affected entity is exactly one hop away.
    Direct,
    /// The affected entity is two or more hops away.
    Transitive,
    /// The affected entity is three or more hops away.
    MultiHop,
    /// The affected entity is an artifact or executable.
    Artifact,
    /// The affected entity is a contract identity.
    Contract,
    /// The affected entity is a deployment record rather than a contract.
    Deployment,
    /// The finding is triggered by a specific typed change.
    Change,
}

impl ImpactType {
    /// The stable wire name, matching the taxonomy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "DIRECT",
            Self::Transitive => "TRANSITIVE",
            Self::MultiHop => "MULTI_HOP",
            Self::Artifact => "ARTIFACT",
            Self::Contract => "CONTRACT",
            Self::Deployment => "DEPLOYMENT",
            Self::Change => "CHANGE",
        }
    }

    /// Which family the term belongs to.
    #[must_use]
    pub const fn family(self) -> ImpactTypeFamily {
        match self {
            Self::Direct | Self::Transitive | Self::MultiHop => ImpactTypeFamily::Distance,
            Self::Artifact | Self::Contract | Self::Deployment => ImpactTypeFamily::Entity,
            Self::Change => ImpactTypeFamily::Trigger,
        }
    }

    /// Whether the term describes how far the change travelled.
    #[must_use]
    pub const fn is_distance(self) -> bool {
        matches!(self.family(), ImpactTypeFamily::Distance)
    }

    /// The least hop depth at which the term is valid.
    ///
    /// `DIRECT`'s value is one and it is also its greatest, which
    /// [`required_distance_terms`] expresses by deriving the whole set from the depth
    /// rather than testing each term's bounds separately.
    #[must_use]
    pub const fn minimum_hop_depth(self) -> usize {
        match self {
            Self::Direct => 1,
            Self::Transitive => 2,
            Self::MultiHop => 3,
            Self::Artifact | Self::Contract | Self::Deployment | Self::Change => 0,
        }
    }

    /// Every entity term the taxonomy defines, for a caller enumerating them.
    #[must_use]
    pub const fn entity_terms() -> &'static [Self] {
        &[Self::Artifact, Self::Contract, Self::Deployment]
    }

    /// Where the term sorts in a canonical classification set.
    ///
    /// Distance first, then entity, then trigger - the order the taxonomy itself
    /// groups the families in. Stated as an explicit rank rather than relying on the
    /// enum's declaration order, because the declaration order is a reading order and a
    /// producer reordering the variants would otherwise silently change every canonical
    /// finding set.
    #[must_use]
    pub const fn rank(self) -> usize {
        match self {
            Self::Direct => 0,
            Self::Transitive => 1,
            Self::MultiHop => 2,
            Self::Artifact => 3,
            Self::Contract => 4,
            Self::Deployment => 5,
            Self::Change => 6,
        }
    }
}

/// The three families `taxonomies/impact-types.yaml` groups its terms into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImpactTypeFamily {
    /// How far the change travelled: `DIRECT`, `TRANSITIVE`, `MULTI_HOP`.
    Distance,
    /// What was affected: `ARTIFACT`, `CONTRACT`, `DEPLOYMENT`.
    Entity,
    /// What triggered the finding: `CHANGE`.
    Trigger,
}

impl ImpactTypeFamily {
    /// The stable wire name, matching the taxonomy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Entity => "entity",
            Self::Trigger => "trigger",
        }
    }
}

/// The distance terms a hop depth requires, exactly.
///
/// The derivation `taxonomies/impact-types.yaml` describes: depth one is `DIRECT`,
/// depth two or more is `TRANSITIVE`, and depth three or more adds `MULTI_HOP` as "a
/// refinement of `TRANSITIVE`". Depth zero has no distance term at all, because a
/// finding whose changed and affected entity are the same was not reached by travelling
/// anywhere.
///
/// Returned as the complete required set rather than as a minimum, so the check is an
/// equality. The taxonomy states the requirement as "MUST include", which would permit
/// a depth-two finding to also claim `DIRECT`; the equality is the stricter reading and
/// the one that makes the classification meaningful, since a term that a finding may
/// attach freely conveys nothing.
#[must_use]
pub fn required_distance_terms(hop_depth: usize) -> Vec<ImpactType> {
    match hop_depth {
        0 => Vec::new(),
        1 => vec![ImpactType::Direct],
        2 => vec![ImpactType::Transitive],
        _ => vec![ImpactType::Transitive, ImpactType::MultiHop],
    }
}

/// The entity term an affected kind requires, if any.
///
/// A `WASM` affected entity requires `ARTIFACT` as well as an `ARTIFACT` entity does,
/// because the taxonomy lists `affectedKinds: [ARTIFACT, WASM]` for that term: an
/// executable is a content-addressed artifact, and a reader asking "which artifacts are
/// affected" must be able to find one.
///
/// `None` for the kinds the taxonomy gives no term - a source, build, package or
/// transaction can be affected without any entity classification being required, and
/// inventing one here would be Amasario adding vocabulary the specification does not
/// define. An entity kind from a later protocol revision reaches the same arm: the
/// engine cannot classify what it has no term for, and guessing one would attach a
/// meaning the specification never assigned.
#[must_use]
pub const fn required_entity_term(kind: EntityKind) -> Option<ImpactType> {
    match kind {
        EntityKind::Contract => Some(ImpactType::Contract),
        EntityKind::Artifact | EntityKind::Wasm => Some(ImpactType::Artifact),
        EntityKind::Deployment => Some(ImpactType::Deployment),
        _ => None,
    }
}

/// The kind of change that triggered a finding.
///
/// Closed, matching `taxonomies/change-types.yaml`. The taxonomy's ordering clause is
/// the important one: `UPGRADED` and `DOWNGRADED` may only be used "where the ordering
/// is established by the referenced ecosystem", and otherwise `MODIFIED` must be used,
/// because "Amasario does not guess ordering". A guess there would invert the direction
/// of an impact assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChangeType {
    /// Present in the after-state and absent from the before-state.
    Added,
    /// Present in the before-state and absent from the after-state.
    Removed,
    /// Present in both under the same identity, with a non-identity property differing.
    Modified,
    /// Changed to a later version, revision or digest under an established ordering.
    Upgraded,
    /// Changed to an earlier version or revision under an established ordering.
    Downgraded,
    /// Changed to a different entity with no meaningful ordering relationship.
    Replaced,
}

impl ChangeType {
    /// The stable wire name, matching the taxonomy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "ADDED",
            Self::Removed => "REMOVED",
            Self::Modified => "MODIFIED",
            Self::Upgraded => "UPGRADED",
            Self::Downgraded => "DOWNGRADED",
            Self::Replaced => "REPLACED",
        }
    }

    /// Whether the relationship between the two states is an ordering.
    ///
    /// `true` only for the two terms the taxonomy restricts to an established ordering.
    /// Exposed so that a producer deciding between `MODIFIED` and `UPGRADED` can ask
    /// the question rather than remembering the rule.
    #[must_use]
    pub const fn claims_ordering(self) -> bool {
        matches!(self, Self::Upgraded | Self::Downgraded)
    }
}

/// Why a path stops where it does.
///
/// Recorded because "a bounded traversal is not an exhausted one". A finding whose path
/// ended at the depth bound and one whose path ended because nothing else propagates
/// look identical from the node list alone, and only the second supports the conclusion
/// that the blast radius has been fully mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PathTermination {
    /// The traversal stopped because the analysis asked it to, and more may exist.
    TargetReached,
    /// The depth bound was reached, so more may exist beyond it.
    MaxDepthReached,
    /// The last entity has no outgoing propagating relationship, so nothing more exists.
    NoPropagatingEdge,
    /// A cycle was detected, so the route was stopped deliberately.
    CycleDetected,
    /// The evidence needed to continue was unavailable at the boundary.
    EvidenceUnavailable,
}

impl PathTermination {
    /// The stable wire name, matching `impact-path.schema.json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TargetReached => "TARGET_REACHED",
            Self::MaxDepthReached => "MAX_DEPTH_REACHED",
            Self::NoPropagatingEdge => "NO_PROPAGATING_EDGE",
            Self::CycleDetected => "CYCLE_DETECTED",
            Self::EvidenceUnavailable => "EVIDENCE_UNAVAILABLE",
        }
    }

    /// Whether the path's end is a limit rather than a fact about the graph.
    ///
    /// The value a consumer most needs: `false` only for
    /// [`PathTermination::NoPropagatingEdge`], which is the one case where the path could
    /// not have continued.
    #[must_use]
    pub const fn is_bound(self) -> bool {
        !matches!(self, Self::NoPropagatingEdge)
    }
}

/// A path from a changed entity to an affected one, with its steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactPath {
    /// The entities on the path, starting at the changed entity and ending at the
    /// affected one. One more entry than there are steps.
    pub nodes: Vec<EntityRef>,
    /// The typed relationships between consecutive nodes.
    pub steps: Vec<Step>,
    /// The number of steps, which is what `hopDepth` means everywhere else.
    pub hop_depth: usize,
    /// Which way the change travelled along the whole path.
    pub direction: Option<ImpactDirection>,
    /// Why the path stops where it does.
    pub termination: Option<PathTermination>,
    /// The path's aggregated confidence: the minimum ordinal across its steps.
    pub confidence: Option<Confidence>,
}

impl ImpactPath {
    /// Builds a path from its entities and the steps between them.
    ///
    /// The node list is derived from the steps rather than accepted, so a path cannot
    /// hold a node sequence that disagrees with the relationships it claims to have
    /// traversed. The only constructible mismatch is an empty step list, which is
    /// rejected because a path with no steps is not a path.
    ///
    /// # Errors
    ///
    /// Returns a validation error when `steps` is empty, or when the first step's source
    /// is not the entity a caller passed as the start.
    pub fn from_steps(start: EntityRef, steps: Vec<Step>) -> Result<Self> {
        let Some(first) = steps.first() else {
            return Err(EngineError::Validation {
                path: "/impact/path/steps".to_owned(),
                detail: "a path needs at least one step; a route with none connects nothing and \
                         cannot be the route to an affected entity"
                    .to_owned(),
            });
        };
        if first.source != start {
            return Err(EngineError::Validation {
                path: "/impact/path/nodes/0".to_owned(),
                detail: format!(
                    "the path starts at {} while its first step starts at {}",
                    start, first.source
                ),
            });
        }
        let mut nodes: Vec<EntityRef> = Vec::with_capacity(steps.len() + 1);
        nodes.push(first.source.clone());
        for step in &steps {
            nodes.push(step.target.clone());
        }
        let hop_depth = steps.len();
        let direction = combine_directions(steps.iter().map(|step| step.direction));
        let confidence = aggregate_confidence(&steps);
        Ok(Self {
            nodes,
            steps,
            hop_depth,
            direction,
            termination: None,
            confidence,
        })
    }

    /// Attaches why the path stopped.
    #[must_use]
    pub const fn with_termination(mut self, termination: PathTermination) -> Self {
        self.termination = Some(termination);
        self
    }

    /// The entity the path starts at.
    #[must_use]
    pub fn start(&self) -> Option<&EntityRef> {
        self.nodes.first()
    }

    /// The entity the path ends at.
    #[must_use]
    pub fn end(&self) -> Option<&EntityRef> {
        self.nodes.last()
    }

    /// The relationships traversed, in order.
    #[must_use]
    pub fn relationships(&self) -> Vec<Relationship> {
        self.steps.iter().map(|step| step.relationship).collect()
    }

    /// Whether the path ends because the graph had nothing further rather than because
    /// a bound stopped it.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.termination
            .is_some_and(|termination| !termination.is_bound())
    }

    /// The path as one line, for a report.
    #[must_use]
    pub fn render(&self) -> String {
        self.steps
            .iter()
            .map(Step::render)
            .collect::<Vec<_>>()
            .join(" -> ")
    }
}

/// The minimum-ordinal aggregation `taxonomies/confidence-levels.yaml` mandates.
///
/// Returns `None` for an empty step list, because there is nothing to aggregate and
/// returning `VERIFIED` - the taxonomy's identity element - would be true of the empty
/// conjunction and badly misleading next to a finding.
///
/// The evidence list is the union of the steps' citations, deduplicated and in step
/// order, so the aggregate cites everything it is derived from. That is the structural
/// half of the specification's invariant that confidence never replaces evidence: the
/// level is computed *from* citations and cannot exist without them.
#[must_use]
pub fn aggregate_confidence(steps: &[Step]) -> Option<Confidence> {
    if steps.is_empty() {
        return None;
    }
    let level = ConfidenceLevel::weakest_of(steps.iter().map(|step| step.confidence.level));
    let mut evidence: Vec<String> = Vec::new();
    let mut contradicting: Vec<String> = Vec::new();
    for step in steps {
        for id in &step.confidence.evidence {
            if !evidence.contains(id) {
                evidence.push(id.clone());
            }
        }
        for id in &step.confidence.contradicting_evidence {
            if !contradicting.contains(id) {
                contradicting.push(id.clone());
            }
        }
    }
    // A step's confidence always cites something, so this cannot be empty; falling back
    // to the step count keeps the type honest rather than panicking on a path that got
    // past construction some other way.
    if evidence.is_empty() {
        evidence.push(format!("{} impact steps", steps.len()));
    }
    Confidence::new(level, evidence, contradicting)
        .ok()
        .map(|confidence| {
            confidence.with_rationale(format!(
                "minimum ordinal across {} step(s), as taxonomies/confidence-levels.yaml requires",
                steps.len()
            ))
        })
}

/// The union of the citations on a path's steps, in step order and deduplicated.
///
/// A finding's evidence list is built from this rather than chosen separately, so the
/// list a reader checks is exactly what the steps cite. Deduplication is by kind and
/// identifier together, because two evidence records of different kinds may share an
/// identifier - a transaction and the observation of it - and the specification treats
/// them as distinct records.
#[must_use]
pub fn path_evidence(steps: &[Step]) -> Vec<EvidenceRef> {
    let mut evidence: Vec<EvidenceRef> = Vec::new();
    for step in steps {
        for citation in &step.evidence {
            if !evidence.contains(citation) {
                evidence.push(citation.clone());
            }
        }
    }
    evidence
}

/// A deployment record with the status impact analysis needs to judge eligibility.
///
/// Separate from [`DeploymentProvenance`] rather than embedded in it because a
/// provenance record says what was observed while a status says how far it has been
/// established, and the two are established by different work. [`Self::from_provenance`]
/// is the bridge, and it deliberately requires the caller to state the status: the
/// engine must not infer `CONFIRMED` from the presence of a transaction, because
/// "a transaction exists" and "the transaction succeeded" are different facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentRecord {
    /// The deployment's entity reference. Its kind must be `DEPLOYMENT`.
    pub entity: EntityRef,
    /// The ledger the deployment was recorded at.
    pub ledger: LedgerSequence,
    /// How far the deployment has been established.
    pub status: DeploymentStatus,
}

impl DeploymentRecord {
    /// Builds a record, rejecting an entity that is not a deployment.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the entity's kind is not `DEPLOYMENT`. A record
    /// whose kind disagreed with its type would let a contract finding be checked
    /// against a deployment's status.
    pub fn new(
        entity: EntityRef,
        ledger: LedgerSequence,
        status: DeploymentStatus,
    ) -> Result<Self> {
        if entity.kind != EntityKind::Deployment {
            return Err(EngineError::Validation {
                path: "/impact/deployment/entity".to_owned(),
                detail: format!(
                    "a deployment record needs a DEPLOYMENT entity, not {}",
                    entity.kind.as_str()
                ),
            });
        }
        Ok(Self {
            entity,
            ledger,
            status,
        })
    }

    /// The record for a provenance claim, at a stated status.
    #[must_use]
    pub fn from_provenance(provenance: &DeploymentProvenance, status: DeploymentStatus) -> Self {
        Self {
            entity: provenance.as_ref(),
            ledger: provenance.ledger,
            status,
        }
    }

    /// Whether a change could affect this deployment.
    #[must_use]
    pub const fn is_eligible(&self) -> bool {
        self.status.eligible_for_impact()
    }
}

/// One statement that a change to some entity may affect another, with its reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactFinding {
    /// The finding's identifier, stable across runs.
    pub id: String,
    /// The entity that changed or is assumed to change.
    pub changed_entity: EntityRef,
    /// The entity potentially affected.
    pub affected_entity: EntityRef,
    /// The finding's classifications.
    pub impact_type: Vec<ImpactType>,
    /// The number of edges between the changed and affected entity.
    pub hop_depth: usize,
    /// The path between them. Required whenever `hop_depth` is greater than zero.
    pub path: Option<ImpactPath>,
    /// The relationships traversed, in path order.
    pub relationship_types: Vec<Relationship>,
    /// Which way the change propagated.
    pub direction: Option<ImpactDirection>,
    /// The kind of change that triggered the finding.
    pub change_type: Option<ChangeType>,
    /// Evidence supporting the finding. Never empty.
    pub evidence: Vec<EvidenceRef>,
    /// How strongly the evidence supports the finding, with its citations.
    pub confidence: Confidence,
    /// Why the entity is considered affected.
    pub reason: String,
    /// Whether the relationships relied on were themselves verified.
    pub verification_state: Option<VerificationStatus>,
    /// The observation boundary the finding was computed at.
    pub boundary: Option<ObservationBoundary>,
    /// Whether propagation was bounded before exhaustion.
    pub truncated: Option<bool>,
}

impl ImpactFinding {
    /// The identifier this engine gives a finding.
    ///
    /// Derived from the changed entity, the affected entity and the change type, so a
    /// re-run over the same graph produces the same identifier and a snapshot diff can
    /// detect a genuinely new finding rather than a reordering. `\u{1f}` separates the
    /// parts because it cannot occur in a contract address, a digest or a change type
    /// name, which plain concatenation could not guarantee.
    #[must_use]
    pub fn stable_id(
        changed: &EntityRef,
        affected: &EntityRef,
        change_type: Option<ChangeType>,
    ) -> String {
        let pre_image = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            changed.kind.as_str(),
            changed.id,
            affected.kind.as_str(),
            affected.id,
        );
        // The change type is appended only when present so that a finding produced
        // without one and the same finding produced for an explicit change are
        // distinguishable, which a diff needs to report the second as new.
        let pre_image = match change_type {
            Some(change_type) => format!("{pre_image}\u{1f}{}", change_type.as_str()),
            None => pre_image,
        };
        Digest::sha256_of(pre_image.as_bytes()).value().to_owned()
    }

    /// Builds a finding, deriving its identifier and its classifications.
    ///
    /// The impact type set is derived rather than accepted: the distance terms from the
    /// hop depth and the entity term from the affected entity's kind, plus `CHANGE` when
    /// a change type is supplied. Deriving it is what makes the classification
    /// trustworthy, because a producer cannot attach a term the depth or the kind does
    /// not support.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the evidence list is empty, since the
    /// specification requires evidence on every finding, or when the confidence cites
    /// nothing.
    pub fn new(
        changed_entity: EntityRef,
        affected_entity: EntityRef,
        hop_depth: usize,
        path: Option<ImpactPath>,
        change_type: Option<ChangeType>,
        evidence: Vec<EvidenceRef>,
        confidence: Confidence,
        reason: impl Into<String>,
    ) -> Result<Self> {
        let mut impact_type = required_distance_terms(hop_depth);
        if let Some(term) = required_entity_term(affected_entity.kind) {
            impact_type.push(term);
        }
        if change_type.is_some() {
            impact_type.push(ImpactType::Change);
        }
        impact_type.sort_by_key(|term| term.rank());
        impact_type.dedup();

        let relationship_types = path
            .as_ref()
            .map(ImpactPath::relationships)
            .unwrap_or_default();
        let direction = path.as_ref().and_then(|path| path.direction);

        let finding = Self {
            id: Self::stable_id(&changed_entity, &affected_entity, change_type),
            changed_entity,
            affected_entity,
            impact_type,
            hop_depth,
            path,
            relationship_types,
            direction,
            change_type,
            evidence,
            confidence,
            reason: reason.into(),
            verification_state: None,
            boundary: None,
            truncated: None,
        };
        finding.validate()?;
        Ok(finding)
    }

    /// Records whether the relationships the finding relies on were verified.
    #[must_use]
    pub const fn with_verification_state(mut self, status: VerificationStatus) -> Self {
        self.verification_state = Some(status);
        self
    }

    /// Records the observation boundary the finding was computed at.
    #[must_use]
    pub fn with_boundary(mut self, boundary: ObservationBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Records that propagation was bounded before exhaustion.
    #[must_use]
    pub const fn with_truncation(mut self, truncated: bool) -> Self {
        self.truncated = Some(truncated);
        self
    }

    /// Whether the affected entity is exactly one hop away.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        self.hop_depth == 1
    }

    /// Whether the affected entity is two or more hops away.
    #[must_use]
    pub const fn is_transitive(&self) -> bool {
        self.hop_depth >= 2
    }

    /// Whether the affected entity is three or more hops away.
    #[must_use]
    pub const fn is_multi_hop(&self) -> bool {
        self.hop_depth >= 3
    }

    /// Whether the finding carries a classification.
    #[must_use]
    pub fn has(&self, impact_type: ImpactType) -> bool {
        self.impact_type.contains(&impact_type)
    }

    /// The finding as one line, for a report's summary.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "{} -> {} ({} hop(s), {})",
            self.changed_entity,
            self.affected_entity,
            self.hop_depth,
            self.impact_type
                .iter()
                .map(|term| term.as_str())
                .collect::<Vec<_>>()
                .join("+")
        )
    }

    /// Every way this finding fails a rule.
    ///
    /// Returns all of them rather than the first, because a report that explained one
    /// defect and hid the rest would send a producer back round the loop for each.
    /// [`Self::validate`] is the convenience that reports the first.
    ///
    /// The checks, in the order the rules state them:
    ///
    /// 1. `impact/direct-impact` - a depth-one finding carries `DIRECT`; every step
    ///    carries evidence and a direction; no step traverses a relationship whose
    ///    change propagation is none.
    /// 2. `impact/transitive-impact` - a depth-two-or-more finding carries `TRANSITIVE`
    ///    and, at three or more, `MULTI_HOP`; its path is present with one more node than
    ///    steps; its relationship list agrees with the path; its aggregated confidence is
    ///    the minimum across its steps.
    /// 3. `impact/deployment-impact` - a deployment finding names a ledger and does not
    ///    name a deployment that cannot be affected.
    /// 4. `impact/change-impact` - `CHANGE` carries a change type, and a zero-depth
    ///    finding is a change finding.
    ///
    /// plus the two invariants the schema states directly: evidence is non-empty, and
    /// every finding includes at least one term from the distance family.
    #[must_use]
    pub fn failures(&self) -> Vec<ImpactFailure> {
        let mut failures: Vec<ImpactFailure> = Vec::new();

        if self.evidence.is_empty() {
            failures.push(ImpactFailure::NoEvidenceCited {
                finding: self.id.clone(),
            });
        }

        // -- distance classification -------------------------------------------
        let required = required_distance_terms(self.hop_depth);
        let claimed: Vec<ImpactType> = self
            .impact_type
            .iter()
            .copied()
            .filter(|term| term.is_distance())
            .collect();
        if required.is_empty() {
            if !claimed.is_empty() {
                failures.push(ImpactFailure::DistanceClassificationDisagrees {
                    hop_depth: self.hop_depth,
                    claimed: render_terms(&claimed),
                    required: "no distance term at depth zero".to_owned(),
                });
            }
        } else if claimed.is_empty() {
            failures.push(ImpactFailure::DistanceClassificationMissing {
                hop_depth: self.hop_depth,
            });
        } else if claimed != required {
            failures.push(ImpactFailure::DistanceClassificationDisagrees {
                hop_depth: self.hop_depth,
                claimed: render_terms(&claimed),
                required: render_terms(&required),
            });
        }

        // -- entity classification ---------------------------------------------
        if let Some(required_term) = required_entity_term(self.affected_entity.kind)
            && !self.has(required_term)
        {
            failures.push(ImpactFailure::EntityClassificationMissing {
                kind: self.affected_entity.kind.as_str().to_owned(),
                required: required_term.as_str().to_owned(),
            });
        }

        // -- change classification ---------------------------------------------
        let claims_change = self.has(ImpactType::Change);
        if claims_change && self.change_type.is_none() {
            failures.push(ImpactFailure::ChangeTypeMissing {
                finding: self.id.clone(),
            });
        }
        if self.hop_depth == 0 && !claims_change {
            failures.push(ImpactFailure::ChangeClassificationDisagrees {
                hop_depth: self.hop_depth,
                claims_change,
                has_change_type: self.change_type.is_some(),
            });
        }

        // -- path ---------------------------------------------------------------
        match &self.path {
            None => {
                if self.hop_depth > 0 {
                    failures.push(ImpactFailure::PathAbsent {
                        finding: self.id.clone(),
                        hop_depth: self.hop_depth,
                    });
                }
            },
            Some(path) => {
                if path.nodes.len() != path.steps.len() + 1 {
                    failures.push(ImpactFailure::PathLengthDisagrees {
                        nodes: path.nodes.len(),
                        steps: path.steps.len(),
                    });
                }
                if path.hop_depth != self.hop_depth {
                    failures.push(ImpactFailure::PathDepthDisagrees {
                        hop_depth: self.hop_depth,
                        path_depth: path.hop_depth,
                    });
                }
                let traversed = path.relationships();
                if traversed != self.relationship_types {
                    failures.push(ImpactFailure::RelationshipListDisagrees {
                        declared: render_relationships(&self.relationship_types),
                        traversed: render_relationships(&traversed),
                    });
                }
                // The derivation is repeated here rather than trusted, so that a
                // direction a producer set by hand is checked against the steps.
                if let Some(derived) = combine_directions(path.steps.iter().map(|s| s.direction))
                    && let Some(declared) = self.direction
                    && declared != derived
                {
                    failures.push(ImpactFailure::DirectionDisagrees {
                        declared: declared.as_str().to_owned(),
                        derived: derived.as_str().to_owned(),
                    });
                }
                let mut seen: Vec<&EntityRef> = Vec::with_capacity(path.nodes.len());
                for node in &path.nodes {
                    if seen.contains(&node) {
                        failures.push(ImpactFailure::PathRepeatsEntity {
                            entity: node.to_string(),
                        });
                        break;
                    }
                    seen.push(node);
                }
                if let Some(aggregate) = aggregate_confidence(&path.steps)
                    && aggregate.level != self.confidence.level
                {
                    failures.push(ImpactFailure::ConfidenceNotWeakestLink {
                        reported: self.confidence.level.as_str().to_owned(),
                        weakest: aggregate.level.as_str().to_owned(),
                    });
                }
                for step in &path.steps {
                    if step.evidence.is_empty() {
                        failures.push(ImpactFailure::StepWithoutEvidence {
                            step: step.render(),
                        });
                    }
                    if !crate::propagation::propagates(step.relationship) {
                        failures.push(ImpactFailure::NonPropagatingStep {
                            relationship: step.relationship.as_str().to_owned(),
                            source: step.source.to_string(),
                            target: step.target.to_string(),
                        });
                    }
                }
            },
        }

        // -- deployment eligibility --------------------------------------------
        if self.affected_entity.kind == EntityKind::Deployment
            && !self.evidence.iter().any(|citation| {
                citation.kind == amasario_core::EvidenceType::Deployment
                    || citation.kind == amasario_core::EvidenceType::Transaction
            })
        {
            failures.push(ImpactFailure::DeploymentLedgerUnrecorded {
                deployment: self.affected_entity.to_string(),
            });
        }

        // -- reason -------------------------------------------------------------
        let length = self.reason.chars().count();
        if length < MINIMUM_REASON_LENGTH {
            failures.push(ImpactFailure::ReasonNotStated { length });
        }

        failures
    }

    /// Validates the finding, reporting the first rule it violates.
    ///
    /// # Errors
    ///
    /// Returns the first failure from [`Self::failures`].
    pub fn validate(&self) -> Result<()> {
        first_failure(self.failures())
    }

    /// Whether the finding passes every structural rule.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.failures().is_empty()
    }
}

fn render_terms(terms: &[ImpactType]) -> String {
    terms
        .iter()
        .map(|term| term.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

fn render_relationships(relationships: &[Relationship]) -> String {
    relationships
        .iter()
        .map(|relationship| relationship.as_str())
        .collect::<Vec<_>>()
        .join(" -> ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::StepDirection;

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a non-empty identifier")
    }

    fn citation(id: &str) -> EvidenceRef {
        EvidenceRef::new(amasario_core::EvidenceType::Transaction, id).expect("a citation")
    }

    fn confidence(level: ConfidenceLevel) -> Confidence {
        Confidence::new(level, vec!["tx-1".to_owned()], Vec::new()).expect("a confidence")
    }

    fn step(
        from: &EntityRef,
        to: &EntityRef,
        relationship: Relationship,
        direction: StepDirection,
        level: ConfidenceLevel,
    ) -> Step {
        Step {
            source: from.clone(),
            target: to.clone(),
            relationship,
            edge_id: format!("edge-{}", relationship.as_str()),
            direction,
            evidence: vec![citation("tx-1")],
            confidence: confidence(level),
        }
    }

    fn direct_finding() -> ImpactFinding {
        let changed = entity(EntityKind::Contract, "C-dependency");
        let affected = entity(EntityKind::Contract, "C-dependent");
        let path = ImpactPath::from_steps(
            changed.clone(),
            vec![step(
                &changed,
                &affected,
                Relationship::DependsOn,
                StepDirection::Inverted,
                ConfidenceLevel::HighConfidence,
            )],
        )
        .expect("a one-step path");
        ImpactFinding::new(
            changed,
            affected,
            1,
            Some(path),
            None,
            vec![citation("tx-1")],
            confidence(ConfidenceLevel::HighConfidence),
            "the affected contract depends on the changed one",
        )
        .expect("a valid direct finding")
    }

    #[test]
    fn the_distance_terms_are_a_function_of_the_depth_and_nothing_else() {
        assert_eq!(required_distance_terms(0), Vec::new());
        assert_eq!(required_distance_terms(1), vec![ImpactType::Direct]);
        assert_eq!(required_distance_terms(2), vec![ImpactType::Transitive]);
        assert_eq!(
            required_distance_terms(3),
            vec![ImpactType::Transitive, ImpactType::MultiHop]
        );
        assert_eq!(
            required_distance_terms(40),
            vec![ImpactType::Transitive, ImpactType::MultiHop],
            "a deep path is still classified by the same rule"
        );
    }

    #[test]
    fn the_entity_terms_are_a_function_of_the_affected_kind() {
        assert_eq!(
            required_entity_term(EntityKind::Contract),
            Some(ImpactType::Contract)
        );
        assert_eq!(
            required_entity_term(EntityKind::Wasm),
            Some(ImpactType::Artifact),
            "an executable is an artifact the taxonomy lists under affectedKinds"
        );
        assert_eq!(
            required_entity_term(EntityKind::Artifact),
            Some(ImpactType::Artifact)
        );
        assert_eq!(
            required_entity_term(EntityKind::Deployment),
            Some(ImpactType::Deployment)
        );
        // The taxonomy defines no term for these, and inventing one would be the engine
        // adding vocabulary the specification does not have.
        for kind in [EntityKind::Source, EntityKind::Build, EntityKind::Package] {
            assert_eq!(required_entity_term(kind), None);
        }
    }

    #[test]
    fn a_direct_finding_derives_its_own_classification() {
        let finding = direct_finding();
        assert_eq!(
            finding.impact_type,
            vec![ImpactType::Direct, ImpactType::Contract],
            "the canonical order is distance, then entity, then trigger"
        );
        assert!(finding.is_direct());
        assert!(!finding.is_transitive());
        assert!(finding.is_valid(), "{:?}", finding.failures());
        assert_eq!(finding.direction, Some(ImpactDirection::Dependents));
        assert_eq!(
            finding.relationship_types,
            vec![Relationship::DependsOn],
            "the relationship list is repeated from the path rather than accepted"
        );
    }

    #[test]
    fn a_finding_whose_confidence_exceeds_its_weakest_step_is_refused() {
        let changed = entity(EntityKind::Contract, "C-dependency");
        let middle = entity(EntityKind::Contract, "C-middle");
        let affected = entity(EntityKind::Build, "build-1");
        let path = ImpactPath::from_steps(
            changed.clone(),
            vec![
                step(
                    &changed,
                    &middle,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::HighConfidence,
                ),
                step(
                    &middle,
                    &affected,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::LowConfidence,
                ),
            ],
        )
        .expect("a two-step path");
        // The path's aggregate is LOW_CONFIDENCE; claiming VERIFIED for the finding is
        // the exact mistake the rule exists to prevent.
        let finding = ImpactFinding::new(
            changed,
            affected,
            2,
            Some(path),
            None,
            vec![citation("tx-1")],
            confidence(ConfidenceLevel::Verified),
            "a build two hops away may be affected",
        )
        .expect_err("an overstated aggregate must be refused");
        assert!(
            finding.to_string().contains("weakest link"),
            "got: {finding}"
        );
    }

    #[test]
    fn a_finding_cannot_reattach_a_distance_term_the_depth_does_not_support() {
        let changed = entity(EntityKind::Contract, "C-a");
        let affected = entity(EntityKind::Contract, "C-b");
        let path = ImpactPath::from_steps(
            changed.clone(),
            vec![
                step(
                    &changed,
                    &affected,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::Verified,
                ),
                step(
                    &affected,
                    &entity(EntityKind::Contract, "C-c"),
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::Verified,
                ),
            ],
        )
        .expect("a two-step path");
        let mut finding = ImpactFinding::new(
            changed,
            entity(EntityKind::Contract, "C-c"),
            2,
            Some(path),
            None,
            vec![citation("tx-1")],
            confidence(ConfidenceLevel::Verified),
            "a contract two hops away may be affected",
        )
        .expect("a valid transitive finding");
        finding.impact_type.push(ImpactType::Direct);
        let failures = finding.failures();
        assert!(
            failures.iter().any(|failure| matches!(
                failure,
                ImpactFailure::DistanceClassificationDisagrees { .. }
            )),
            "got: {failures:?}"
        );
    }

    #[test]
    fn an_observational_step_can_never_appear_in_a_path() {
        let changed = entity(EntityKind::Contract, "C-a");
        let affected = entity(EntityKind::Transaction, "tx-1");
        let path = ImpactPath::from_steps(
            changed.clone(),
            vec![step(
                &changed,
                &affected,
                Relationship::ObservedIn,
                StepDirection::Forward,
                ConfidenceLevel::MediumConfidence,
            )],
        )
        .expect("a one-step path");
        // The refusal happens at construction, not at report time, so a path containing
        // an observational link cannot be built in the first place.
        let error = ImpactFinding::new(
            changed,
            affected,
            1,
            Some(path),
            None,
            vec![citation("tx-1")],
            confidence(ConfidenceLevel::MediumConfidence),
            "the transaction observed the contract",
        )
        .expect_err("traversing OBSERVED_IN must be refused");
        let message = error.to_string();
        assert!(
            message.contains("change propagation is none"),
            "got: {message}"
        );
        assert!(message.contains("OBSERVED_IN"), "got: {message}");
    }

    #[test]
    fn a_zero_depth_finding_must_be_a_change_finding() {
        let entity_ref = entity(EntityKind::Contract, "C-a");
        let error = ImpactFinding::new(
            entity_ref.clone(),
            entity_ref,
            0,
            None,
            None,
            vec![citation("tx-1")],
            confidence(ConfidenceLevel::HighConfidence),
            "the contract itself is what changed",
        )
        .expect_err("a zero-depth finding without CHANGE must be refused");
        assert!(error.to_string().contains("CHANGE"), "got: {error}");
    }

    #[test]
    fn a_change_finding_names_its_change_and_classifies_its_entity() {
        let entity_ref = entity(EntityKind::Wasm, &"ab".repeat(32));
        let finding = ImpactFinding::new(
            entity_ref.clone(),
            entity_ref,
            0,
            None,
            Some(ChangeType::Replaced),
            vec![citation("tx-1")],
            confidence(ConfidenceLevel::HighConfidence),
            "the deployed executable at this digest was replaced",
        )
        .expect("a valid change finding");
        assert_eq!(
            finding.impact_type,
            vec![ImpactType::Artifact, ImpactType::Change]
        );
        assert_eq!(finding.change_type, Some(ChangeType::Replaced));
        assert!(finding.is_valid(), "{:?}", finding.failures());
        // The taxonomy allows UPGRADED only under an established ordering; MODIFIED is
        // the term for a change with none, and REPLACED for one with no relevant order.
        assert!(!ChangeType::Modified.claims_ordering());
        assert!(ChangeType::Upgraded.claims_ordering());
    }

    #[test]
    fn the_finding_identifier_is_stable_and_changes_when_the_claim_does() {
        let changed = entity(EntityKind::Contract, "C-a");
        let affected = entity(EntityKind::Contract, "C-b");
        let first = ImpactFinding::stable_id(&changed, &affected, None);
        assert_eq!(first, ImpactFinding::stable_id(&changed, &affected, None));
        assert_ne!(
            first,
            ImpactFinding::stable_id(&changed, &affected, Some(ChangeType::Modified)),
            "a change-triggered finding is a different finding"
        );
        assert_ne!(
            first,
            ImpactFinding::stable_id(&affected, &changed, None),
            "the direction of the claim is part of its identity"
        );
    }

    #[test]
    fn the_path_aggregate_is_the_minimum_ordinal_across_its_steps() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let path = ImpactPath::from_steps(
            a.clone(),
            vec![
                step(
                    &a,
                    &b,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::Verified,
                ),
                step(
                    &b,
                    &c,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::LowConfidence,
                ),
            ],
        )
        .expect("a two-step path");
        let aggregate = path.confidence.expect("an aggregate");
        assert_eq!(aggregate.level, ConfidenceLevel::LowConfidence);
        assert!(
            aggregate
                .rationale
                .as_deref()
                .is_some_and(|rationale| rationale.contains("minimum ordinal")),
            "an aggregate must say how it was computed"
        );
        assert_eq!(path.direction, Some(ImpactDirection::Dependents));
    }

    #[test]
    fn a_route_that_changes_course_is_reported_as_having_gone_both_ways() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let path = ImpactPath::from_steps(
            a.clone(),
            vec![
                step(
                    &a,
                    &b,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::Verified,
                ),
                step(
                    &b,
                    &c,
                    Relationship::Affects,
                    StepDirection::Forward,
                    ConfidenceLevel::Verified,
                ),
            ],
        )
        .expect("a two-step path");
        assert_eq!(path.direction, Some(ImpactDirection::Both));
    }

    #[test]
    fn a_path_whose_first_step_starts_elsewhere_is_refused() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let error = ImpactPath::from_steps(
            a,
            vec![step(
                &b,
                &entity(EntityKind::Contract, "C-c"),
                Relationship::DependsOn,
                StepDirection::Inverted,
                ConfidenceLevel::Verified,
            )],
        )
        .expect_err("a mismatched start must be refused");
        assert!(error.to_string().contains("first step starts at"));
    }

    #[test]
    fn a_deployment_record_refuses_an_entity_that_is_not_a_deployment() {
        let error = DeploymentRecord::new(
            entity(EntityKind::Contract, "C-a"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            DeploymentStatus::Confirmed,
        )
        .expect_err("a contract is not a deployment record");
        assert!(error.to_string().contains("DEPLOYMENT entity"));
    }

    #[test]
    fn only_deployments_that_took_effect_are_eligible() {
        let ledger = LedgerSequence::new(1_000).expect("a real ledger");
        let deployment = entity(EntityKind::Deployment, "D-1");
        for status in [DeploymentStatus::Observed, DeploymentStatus::Confirmed] {
            let record = DeploymentRecord::new(deployment.clone(), ledger, status)
                .expect("a deployment record");
            assert!(record.is_eligible(), "{status} took effect");
        }
        for status in [
            DeploymentStatus::Unconfirmed,
            DeploymentStatus::Failed,
            DeploymentStatus::Unknown,
        ] {
            let record = DeploymentRecord::new(deployment.clone(), ledger, status)
                .expect("a deployment record");
            assert!(!record.is_eligible(), "{status} has not been established");
        }
    }

    #[test]
    fn a_path_knows_whether_it_stopped_because_it_had_to() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let build = || {
            ImpactPath::from_steps(
                a.clone(),
                vec![step(
                    &a,
                    &b,
                    Relationship::DependsOn,
                    StepDirection::Inverted,
                    ConfidenceLevel::Verified,
                )],
            )
            .expect("a one-step path")
        };
        assert!(
            !build()
                .with_termination(PathTermination::MaxDepthReached)
                .is_complete()
        );
        assert!(
            build()
                .with_termination(PathTermination::NoPropagatingEdge)
                .is_complete()
        );
        assert!(PathTermination::MaxDepthReached.is_bound());
        assert!(!PathTermination::NoPropagatingEdge.is_bound());
    }

    #[test]
    fn an_aggregate_over_no_steps_is_absent_rather_than_verified() {
        // `VERIFIED` is the identity element of the aggregation, so returning it here
        // would report the empty conjunction as the strongest possible evidence.
        assert!(aggregate_confidence(&[]).is_none());
    }
}
