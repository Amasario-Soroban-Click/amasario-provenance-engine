//! Which way a change travels, and which relationships let it travel at all.
//!
//! # The one place impact is derived from
//!
//! `taxonomies/relationship-types.yaml` declares a `changePropagation` for every
//! relationship and states, in the taxonomy itself, that "impact analysis is derived
//! from `changePropagation`; it is never inferred from the name of the relationship."
//! This module is that derivation and nothing else. Every traversal in this crate asks
//! [`propagates_forward`] or [`propagates_backward`], so a relationship that changes
//! its propagation in the taxonomy changes the analysis in one place rather than in
//! whichever functions happened to hardcode a direction.
//!
//! # Reading a relationship as an arrow
//!
//! An edge is `subject -RELATIONSHIP-> object`. Two of the taxonomy's three
//! propagating values travel against the arrow and one travels with it:
//!
//! | `changePropagation` | A change to the object reaches | A change to the subject reaches |
//! | --- | --- | --- |
//! | `object_to_subject` | the subject | nothing |
//! | `subject_to_object` | nothing | the object |
//! | `bidirectional` | the subject | the object |
//! | `none` | nothing | nothing |
//!
//! `DEPENDS_ON` is `object_to_subject`, which is why a change to a dependency reaches
//! its dependents and a change to a dependent reaches nothing upstream. That inversion
//! is the whole reason the direction is declared rather than guessed: `DEPENDS_ON`
//! reads subject-first, so a name-based implementation would propagate the wrong way.
//!
//! # Why `none` is a refusal rather than a no-op
//!
//! `OBSERVED_IN` and `VERIFIED_BY` have `changePropagation: none`, and the taxonomy
//! explains why: "a re-observation does not change the fact" and "a re-verification is
//! not a change". Silently skipping such an edge would be tolerable if it were the only
//! consequence, but a skipped edge and an absent edge produce the same empty result -
//! and a reader cannot tell "there is no impact" from "the analysis declined to look".
//! The crate therefore makes traversing one a named failure
//! ([`crate::ImpactFailure::NonPropagatingStep`]) and never skips quietly.

use amasario_core::{ChangePropagation, Confidence, EntityRef, ObservationBoundary, Relationship};
use amasario_dependency::{Dependency, EvidenceRef};
use amasario_graph::Graph;

/// Whether a change travelled with the arrow a relationship points along.
///
/// The distinction is not cosmetic. `AFFECTS` is `subject_to_object`, so a change to
/// its subject reaches its object *forward*; `DEPENDS_ON` is `object_to_subject`, so a
/// change to its object reaches its subject by reading the same edge *inverted*. A
/// consumer that recorded only the node sequence could not tell which of the two
/// happened, which is why the path schema requires the direction on every step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StepDirection {
    /// The change travelled from the subject to the object, with the arrow.
    Forward,
    /// The change travelled from the object to the subject, against the arrow.
    Inverted,
}

impl StepDirection {
    /// The stable wire name, matching `impact-path.schema.json`'s step enumeration.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Inverted => "inverted",
        }
    }

    /// Whether the change travelled against the arrow.
    #[must_use]
    pub const fn is_inverted(self) -> bool {
        matches!(self, Self::Inverted)
    }
}

/// Which way the change propagated across a whole path.
///
/// The specification's `impact.schema.json` defines the three values as `DEPENDENTS`
/// when "the changed entity was traversed to its dependents", `DEPENDENCIES` when
/// "the traversal followed the changed entity's own dependencies", and `BOTH` when both
/// were followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImpactDirection {
    /// The traversal moved towards the entities that depend on the changed one.
    Dependents,
    /// The traversal moved towards the entities the changed one depends on.
    Dependencies,
    /// The traversal did both.
    Both,
}

impl ImpactDirection {
    /// The stable wire name, matching `impact.schema.json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dependents => "DEPENDENTS",
            Self::Dependencies => "DEPENDENCIES",
            Self::Both => "BOTH",
        }
    }

    /// The contribution one step makes to a path's direction.
    ///
    /// An inverted step reaches an entity that depends on the changed one; a forward
    /// step reaches one the changed entity depends on. Kept as the inverse of
    /// [`StepDirection`] rather than a second decision so that the two cannot drift.
    #[must_use]
    pub const fn of_step(step: StepDirection) -> Self {
        match step {
            StepDirection::Inverted => Self::Dependents,
            StepDirection::Forward => Self::Dependencies,
        }
    }
}

/// Combines the directions of several steps into one path direction.
///
/// `None` for an empty sequence, because a path with no steps has no direction and
/// returning `Both` would attribute a direction to nothing. A mixed sequence is
/// [`ImpactDirection::Both`], which is the honest answer: the specification provides
/// the value precisely so that a route that changed course is not silently described as
/// having gone one way.
#[must_use]
pub fn combine_directions(
    steps: impl IntoIterator<Item = StepDirection>,
) -> Option<ImpactDirection> {
    let mut combined: Option<ImpactDirection> = None;
    for step in steps {
        let contribution = ImpactDirection::of_step(step);
        combined = Some(match combined {
            None => contribution,
            Some(existing) if existing == contribution => existing,
            Some(_) => ImpactDirection::Both,
        });
    }
    combined
}

/// Whether a change to a relationship's object reaches its subject.
///
/// This is the question `changePropagation` answers for `object_to_subject`, which is
/// the value five of the eight relationships declare.
#[must_use]
pub const fn propagates_backward(relationship: Relationship) -> bool {
    relationship
        .change_propagation()
        .propagates_object_to_subject()
}

/// Whether a change to a relationship's subject reaches its object.
#[must_use]
pub const fn propagates_forward(relationship: Relationship) -> bool {
    relationship
        .change_propagation()
        .propagates_subject_to_object()
}

/// Whether a change travels along a relationship at all.
///
/// `false` only for `OBSERVED_IN` and `VERIFIED_BY`. Exposed so that a caller
/// inspecting a path can ask before traversing rather than after failing.
#[must_use]
pub const fn propagates(relationship: Relationship) -> bool {
    relationship.change_propagation().propagates()
}

/// Why a relationship does not carry a change, in the taxonomy's terms.
///
/// Returns `None` when it does carry one. Stated here rather than left to a caller to
/// interpolate, because the two non-propagating relationships do not fail for the same
/// reason and a report that said "none" for both would lose the distinction the
/// taxonomy draws.
#[must_use]
pub const fn non_propagation_reason(relationship: Relationship) -> Option<&'static str> {
    match relationship.change_propagation() {
        ChangePropagation::None => match relationship {
            Relationship::ObservedIn => Some(
                "re-observing a fact does not change the fact, so a change cannot travel from a \
                 transaction to the entity observed in it",
            ),
            Relationship::VerifiedBy => Some(
                "a re-verification is not a change to the thing verified, so a change cannot \
                 travel from the verifying record to the claim it supports",
            ),
            _ => Some("this relationship declares that a change does not propagate through it"),
        },
        _ => None,
    }
}

/// Everything a traversal needs, so that the traversal functions take one argument.
///
/// Gathered into a context rather than threaded through every call because the three
/// pieces have to move together: the exclusion list is derived from the deployments in
/// the same run, and a path's boundary is the boundary the graph was assembled at. A
/// caller that passed one and forgot another would produce findings attributed to the
/// wrong observation, which is not detectable from the findings themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactContext<'a> {
    /// The graph to traverse.
    pub graph: &'a Graph,
    /// Entities a change must not be propagated into.
    ///
    /// The analyzer passes the deployments that have not been established as having
    /// taken effect. Excluding them from traversal rather than filtering them out of the
    /// result matters: the executable a failed deployment would have installed is not
    /// the one at that address, so nothing behind it can be reached either.
    pub excluded: Vec<EntityRef>,
    /// The boundary the analysis was computed at, where one was recorded.
    pub boundary: Option<ObservationBoundary>,
}

impl<'a> ImpactContext<'a> {
    /// A context over a graph, excluding nothing.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self {
            graph,
            excluded: Vec::new(),
            boundary: graph.boundary.clone(),
        }
    }

    /// Adds an entity that must not be traversed into.
    #[must_use]
    pub fn excluding(mut self, entity: EntityRef) -> Self {
        if !self.excluded.contains(&entity) {
            self.excluded.push(entity);
        }
        self
    }

    /// Records the boundary the analysis was computed at.
    #[must_use]
    pub fn with_boundary(mut self, boundary: ObservationBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Whether an entity is excluded from traversal.
    #[must_use]
    pub fn excludes(&self, entity: &EntityRef) -> bool {
        self.excluded.contains(entity)
    }

    /// The steps a change can take out of an entity, in canonical order.
    ///
    /// Empty for an excluded entity. An exclusion therefore has two effects that are
    /// really one rule: nothing propagates into an excluded entity, and nothing
    /// propagates out of one either. The second is what makes the rule useful for
    /// deployments: a deployment that never took effect cannot be a source of impact, so
    /// the executable behind it is not reachable from it.
    #[must_use]
    pub fn steps_from(&self, from: &EntityRef) -> Vec<Step> {
        if self.excludes(from) {
            return Vec::new();
        }
        steps_from(self.graph, from, &self.excluded)
    }
}

/// One traversed relationship, with everything a reader needs to check it.
///
/// Deliberately not the graph's [`amasario_graph::Edge`]: an edge is a stable fact about
/// the graph, while a step is a claim about how a particular analysis moved through it,
/// including the direction. An edge has no direction because it is not a traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The entity the step starts at, which is where the change had reached.
    pub source: EntityRef,
    /// The entity the step ends at, which is what the change reached next.
    pub target: EntityRef,
    /// The relationship traversed.
    pub relationship: Relationship,
    /// The graph edge this step corresponds to.
    pub edge_id: String,
    /// Whether the change travelled with the arrow or against it.
    pub direction: StepDirection,
    /// The evidence the edge cites, carried onto the step.
    ///
    /// Required non-empty by rule `impact/transitive-impact`, which is why a step is
    /// built only from an edge that has some: an edge without evidence cannot become a
    /// step, so a path cannot contain an unsupported link.
    pub evidence: Vec<EvidenceRef>,
    /// The edge's own confidence, with its citations.
    pub confidence: Confidence,
}

impl Step {
    /// The step as one line, for a report or an error message.
    #[must_use]
    pub fn render(&self) -> String {
        let arrow = if self.direction.is_inverted() {
            "<-"
        } else {
            "->"
        };
        format!(
            "{} {arrow} {} {}",
            self.source,
            self.relationship.as_str(),
            self.target
        )
    }
}

/// Every way a change can leave an entity, derived from the declared propagation.
///
/// Returns the steps in canonical order: by the edge's canonical ordering, which the
/// graph layer already defines. Canonical order matters because two runs over the same
/// graph must produce the same finding identifiers, and an order that depended on
/// insertion would make the identifiers unstable.
///
/// `excluded` names entities that must not be traversed into. The analyzer passes the
/// deployments that have not been established as having taken effect, because a change
/// cannot reach a deployment that never happened - and, more importantly, nothing beyond
/// it either, since the executable it would have hosted is not the one at that address.
#[must_use]
pub fn steps_from(graph: &Graph, from: &EntityRef, excluded: &[EntityRef]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();

    for edge in &graph.edges {
        let dependency: &Dependency = edge.dependency();
        let relationship = dependency.relationship;
        let Some(direction) =
            step_direction(relationship, &dependency.subject, &dependency.object, from)
        else {
            continue;
        };
        let target = match direction {
            StepDirection::Forward => &dependency.object,
            StepDirection::Inverted => &dependency.subject,
        };
        if excluded.contains(target) {
            continue;
        }
        // An edge with no evidence cannot become a step. The rule requires evidence on
        // every step, so admitting one here would move the failure from construction to
        // validation, where it would be reported against the path rather than against
        // the edge that lacked the evidence.
        if dependency.evidence.is_empty() {
            continue;
        }
        steps.push(Step {
            source: from.clone(),
            target: target.clone(),
            relationship,
            edge_id: edge.id().to_owned(),
            direction,
            evidence: dependency.evidence.clone(),
            confidence: dependency.confidence.clone(),
        });
    }

    steps.sort_by(|left, right| {
        left.target
            .kind
            .cmp(&right.target.kind)
            .then_with(|| left.target.id.cmp(&right.target.id))
            .then_with(|| left.relationship.cmp(&right.relationship))
            .then_with(|| left.edge_id.cmp(&right.edge_id))
    });
    steps.dedup_by(|left, right| {
        left.target == right.target
            && left.relationship == right.relationship
            && left.edge_id == right.edge_id
    });
    steps
}

/// States why a change reached an entity, in terms of the relationships traversed.
///
/// This is the field that replaces a score. `schema/impact.schema.json` requires a
/// reason "stated in terms of the traversed relationships so that a reader can follow
/// it", and the shape matters more than the wording: the sentence names the changed
/// entity, the affected one, the number of hops, and every relationship with the
/// direction it was read in. A reader who disagrees can point at the step they dispute.
///
/// Deterministic from its inputs, so two runs over the same graph produce the same
/// bytes and a snapshot diff does not report a finding as changed because its prose
/// was rebuilt differently.
#[must_use]
pub fn describe_route(changed: &EntityRef, affected: &EntityRef, steps: &[Step]) -> String {
    let route = steps
        .iter()
        .map(Step::render)
        .collect::<Vec<_>>()
        .join(", then ");
    let hops = steps.len();
    let hop_word = if hops == 1 { "hop" } else { "hops" };
    format!("a change to {changed} reaches {affected} in {hops} {hop_word}: {route}")
}

/// Which way a change from `from` travels across one dependency, if it travels at all.
///
/// Returns `None` when the change does not reach the other end. Note that a
/// bidirectional relationship can be traversed either way, so the answer depends on
/// which end `from` is; that is why the question takes the endpoints rather than only
/// the relationship.
#[must_use]
pub fn step_direction(
    relationship: Relationship,
    subject: &EntityRef,
    object: &EntityRef,
    from: &EntityRef,
) -> Option<StepDirection> {
    if from == subject && propagates_forward(relationship) {
        return Some(StepDirection::Forward);
    }
    // A self-edge would be reported as forward, which is misleading: the two ends are
    // the same entity, so no direction describes the move. The graph layer refuses a
    // self-dependency outright, so this is a defensive branch rather than a live one -
    // but it is the branch that keeps the answer well defined if that ever changed.
    if from == object && from != subject && propagates_backward(relationship) {
        return Some(StepDirection::Inverted);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{ConfidenceLevel, EntityKind};

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a non-empty identifier")
    }

    #[test]
    fn the_taxonomy_declares_exactly_two_relationships_that_carry_no_change() {
        // Spelled out rather than counted, so that a relationship whose propagation the
        // taxonomy changes fails here rather than silently altering an analysis.
        assert!(!propagates(Relationship::ObservedIn));
        assert!(!propagates(Relationship::VerifiedBy));
        for relationship in [
            Relationship::DependsOn,
            Relationship::Invocates,
            Relationship::BuiltFrom,
            Relationship::DerivedFrom,
            Relationship::DeployedAs,
            Relationship::Affects,
        ] {
            assert!(
                propagates(relationship),
                "{} should carry a change",
                relationship.as_str()
            );
        }
    }

    #[test]
    fn dependency_propagates_against_its_arrow_and_affects_along_it() {
        // The inversion that makes the whole module necessary: `DEPENDS_ON` reads
        // subject-first, but a change to its object is what reaches its subject.
        assert!(propagates_backward(Relationship::DependsOn));
        assert!(!propagates_forward(Relationship::DependsOn));
        assert!(propagates_forward(Relationship::Affects));
        assert!(!propagates_backward(Relationship::Affects));
    }

    #[test]
    fn a_change_to_a_dependent_does_not_reach_the_dependency_it_uses() {
        let subject = entity(EntityKind::Contract, "C-dependent");
        let object = entity(EntityKind::Contract, "C-dependency");
        // `subject DEPENDS_ON object`. From the subject, nothing propagates: a change to
        // something that uses a library does not change the library.
        assert_eq!(
            step_direction(Relationship::DependsOn, &subject, &object, &subject),
            None
        );
        // From the object, the change reaches the subject, against the arrow.
        assert_eq!(
            step_direction(Relationship::DependsOn, &subject, &object, &object),
            Some(StepDirection::Inverted)
        );
    }

    #[test]
    fn a_change_to_an_affects_subject_reaches_its_object_with_the_arrow() {
        let subject = entity(EntityKind::Contract, "C-a");
        let object = entity(EntityKind::Wasm, "ab".repeat(32).as_str());
        assert_eq!(
            step_direction(Relationship::Affects, &subject, &object, &subject),
            Some(StepDirection::Forward)
        );
        assert_eq!(
            step_direction(Relationship::Affects, &subject, &object, &object),
            None
        );
    }

    #[test]
    fn an_unrelated_entity_has_no_direction_across_a_relationship() {
        let subject = entity(EntityKind::Contract, "C-a");
        let object = entity(EntityKind::Contract, "C-b");
        let stranger = entity(EntityKind::Contract, "C-c");
        assert_eq!(
            step_direction(Relationship::DependsOn, &subject, &object, &stranger),
            None
        );
    }

    #[test]
    fn direction_combination_reports_both_only_when_a_route_actually_turns() {
        assert_eq!(
            combine_directions([StepDirection::Inverted, StepDirection::Inverted]),
            Some(ImpactDirection::Dependents)
        );
        assert_eq!(
            combine_directions([StepDirection::Forward, StepDirection::Forward]),
            Some(ImpactDirection::Dependencies)
        );
        assert_eq!(
            combine_directions([StepDirection::Inverted, StepDirection::Forward]),
            Some(ImpactDirection::Both)
        );
        // No steps is not a direction. `Both` would claim the route went two ways.
        assert_eq!(combine_directions([]), None);
    }

    #[test]
    fn the_non_propagation_reasons_are_different_for_the_two_relationships() {
        let observed = non_propagation_reason(Relationship::ObservedIn).expect("a reason");
        let verified = non_propagation_reason(Relationship::VerifiedBy).expect("a reason");
        assert_ne!(
            observed, verified,
            "the taxonomy gives the two different rationales and a report should keep both"
        );
        assert!(observed.contains("does not change the fact"));
        assert!(verified.contains("re-verification is not a change"));
        assert_eq!(non_propagation_reason(Relationship::DependsOn), None);
    }

    #[test]
    fn a_step_renders_the_direction_so_a_reader_can_see_the_inversion() {
        let step = Step {
            source: entity(EntityKind::Contract, "C-a"),
            target: entity(EntityKind::Contract, "C-b"),
            relationship: Relationship::DependsOn,
            edge_id: "edge-1".to_owned(),
            direction: StepDirection::Inverted,
            evidence: vec![
                EvidenceRef::new(amasario_core::EvidenceType::Transaction, "tx-1")
                    .expect("a citation"),
            ],
            confidence: Confidence::new(
                ConfidenceLevel::HighConfidence,
                vec!["tx-1".to_owned()],
                Vec::new(),
            )
            .expect("a confidence"),
        };
        assert_eq!(step.render(), "CONTRACT:C-a <- DEPENDS_ON CONTRACT:C-b");
        assert!(step.direction.is_inverted());
    }
}
