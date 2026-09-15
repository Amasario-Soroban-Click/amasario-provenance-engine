//! Multi-hop impact: how far a change reaches, and what the traversal had to stop at.
//!
//! # Why the traversal is bounded and says so
//!
//! `rules/transitive-impact.yaml` ends with the non-goal that shapes this module: "This
//! rule does not claim that every reachable entity is affected, only those the bounded
//! traversal found." A cycle, a deep chain and a dense fan-out all make an exhaustive
//! answer expensive, and - more to the point - an exhaustive answer is not what an
//! operator needs before an upgrade. What they need is a route to each entity they might
//! care about, and a statement of whether the search ran out of entities or out of
//! budget. [`Propagation::truncated`] and [`Propagation::truncation_reason`] are that
//! statement, and they are carried rather than recomputed because a bounded search and a
//! complete one over the same graph produce identical findings.
//!
//! # Breadth first, so the shortest route wins
//!
//! Each entity is reported at the depth it was first reached at, which is its shortest
//! route from the changed entity. That matters because the finding's hop depth decides
//! its classification and its aggregate confidence: reporting an entity by a longer
//! route would classify it as `MULTI_HOP` when a one-hop route exists, and would report a
//! weaker confidence than the graph supports.
//!
//! # Cycles are reported, not removed
//!
//! The traversal never revisits an entity, so a path cannot repeat one and every route
//! is acyclic by construction. A cycle is not thereby hidden: when a candidate step
//! points back into the route that produced it, the route is marked
//! [`PathTermination::CycleDetected`] rather than silently closed. The dependency layer
//! makes the same choice, for the same reason - a consumer that never sees a cycle
//! cannot know the graph was not a tree.

use std::collections::VecDeque;

use amasario_core::{EntityRef, ObservationBoundary, TruncationReason};
use amasario_dependency::Limits;

use crate::affected::{ChangeType, ImpactFinding, ImpactPath, PathTermination, path_evidence};
use crate::propagation::{ImpactContext, Step, describe_route};

/// The route a change took to one entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// The entity the change reached. Never the changed entity itself.
    pub entity: EntityRef,
    /// How many steps the route takes, which is the finding's hop depth.
    pub depth: usize,
    /// The steps, in order, from the changed entity to this one.
    pub steps: Vec<Step>,
    /// Why the route stops here.
    pub termination: PathTermination,
}

impl Reach {
    /// The entities the route passes through, the changed entity first.
    #[must_use]
    pub fn nodes(&self) -> Vec<EntityRef> {
        let mut nodes: Vec<EntityRef> = Vec::with_capacity(self.steps.len() + 1);
        if let Some(first) = self.steps.first() {
            nodes.push(first.source.clone());
        }
        for step in &self.steps {
            nodes.push(step.target.clone());
        }
        nodes
    }

    /// Whether the route is two or more hops.
    #[must_use]
    pub const fn is_transitive(&self) -> bool {
        self.depth >= 2
    }

    /// Whether the route is three or more hops.
    #[must_use]
    pub const fn is_multi_hop(&self) -> bool {
        self.depth >= 3
    }
}

/// What a bounded propagation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Propagation {
    /// The entity the change started at.
    pub start: EntityRef,
    /// What it reached, in canonical order: by depth, then entity kind, then identifier.
    pub reaches: Vec<Reach>,
    /// The bounds it ran under.
    pub limits: Limits,
    /// The boundary the graph was assembled at, so that a finding can carry it.
    pub boundary: Option<ObservationBoundary>,
    /// Whether traversal stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why it stopped, when it did.
    pub truncation_reason: Option<TruncationReason>,
    /// How many entities were visited, including the start.
    pub nodes_visited: usize,
    /// The greatest depth any route reached.
    pub deepest: usize,
    /// How many routes ended at a cycle rather than at an exhausted graph.
    pub cycles_encountered: usize,
}

impl Propagation {
    /// Whether anything was reached.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.reaches.is_empty()
    }

    /// How many entities were reached.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.reaches.len()
    }

    /// Whether an empty result is a negative finding rather than an inconclusive one.
    ///
    /// The distinction the whole bounded-search vocabulary exists for: nothing reached in
    /// a complete search means a change reaches nothing, and nothing reached in a bounded
    /// one means nothing was found within the bound.
    #[must_use]
    pub const fn is_conclusive(&self) -> bool {
        !self.truncated
    }

    /// The route that reached an entity, if one did.
    #[must_use]
    pub fn reach(&self, entity: &EntityRef) -> Option<&Reach> {
        self.reaches.iter().find(|reach| reach.entity == *entity)
    }

    /// The depth an entity was reached at, if it was reached.
    #[must_use]
    pub fn depth_of(&self, entity: &EntityRef) -> Option<usize> {
        self.reach(entity).map(|reach| reach.depth)
    }

    /// The routes at exactly one depth.
    #[must_use]
    pub fn at_depth(&self, depth: usize) -> Vec<&Reach> {
        self.reaches
            .iter()
            .filter(|reach| reach.depth == depth)
            .collect()
    }

    /// The routes showing a cycle was hit.
    #[must_use]
    pub fn cyclic_routes(&self) -> Vec<&Reach> {
        self.reaches
            .iter()
            .filter(|reach| reach.termination == PathTermination::CycleDetected)
            .collect()
    }

    /// The entities reached, in canonical order.
    #[must_use]
    pub fn entities(&self) -> Vec<&EntityRef> {
        self.reaches.iter().map(|reach| &reach.entity).collect()
    }

    /// The findings for this propagation.
    ///
    /// One per reached entity, each carrying its route, its derived classification, its
    /// aggregate confidence and the evidence its steps cite. Canonically ordered, so two
    /// runs over one graph produce the same list.
    #[must_use]
    pub fn findings(&self, change_type: Option<ChangeType>) -> Vec<ImpactFinding> {
        let mut findings: Vec<ImpactFinding> = Vec::with_capacity(self.reaches.len());
        for reach in &self.reaches {
            let Ok(path) = ImpactPath::from_steps(self.start.clone(), reach.steps.clone()) else {
                continue;
            };
            let path = path.with_termination(reach.termination);
            let Some(confidence) = path.confidence.clone() else {
                continue;
            };
            let evidence = path_evidence(&path.steps);
            let reason = describe_route(&self.start, &reach.entity, &path.steps);
            let Ok(finding) = ImpactFinding::new(
                self.start.clone(),
                reach.entity.clone(),
                reach.depth,
                Some(path),
                change_type,
                evidence,
                confidence,
                reason,
            ) else {
                continue;
            };
            // Truncation and the boundary belong to the run that produced the finding,
            // so they are attached here rather than left to a caller to remember.
            let finding = finding.with_truncation(self.truncated);
            let finding = match &self.boundary {
                Some(boundary) => finding.with_boundary(boundary.clone()),
                None => finding,
            };
            findings.push(finding);
        }
        findings
    }
}

/// Propagates a change from one entity as far as the bounds allow.
///
/// Breadth first over the steps each entity can take, never revisiting an entity. The
/// result is canonical: entities are reported in the order they were discovered, then
/// sorted by depth, kind and identifier, so a re-run produces identical findings.
#[must_use]
pub fn propagate(context: &ImpactContext<'_>, changed: &EntityRef, limits: Limits) -> Propagation {
    let start = changed.clone();
    let mut reached: Vec<Reach> = Vec::new();
    let mut visited: Vec<EntityRef> = vec![start.clone()];
    // A queue, not a stack: first in, first out is what makes each entity's first visit
    // its shortest route. A stack would explore one branch to its end and report
    // entities at whatever depth that branch happened to find them.
    let mut frontier: VecDeque<(EntityRef, Vec<Step>, usize)> =
        VecDeque::from([(start.clone(), Vec::new(), 0)]);
    let mut truncated = false;
    let mut truncation_reason: Option<TruncationReason> = None;
    let mut deepest = 0;
    let mut cycles_encountered = 0;

    while let Some((entity, steps, depth)) = frontier.pop_front() {
        if depth >= limits.max_depth {
            // Only a bound if there was somewhere further to go. An entity at the bound
            // with no propagating step left is the end of the graph, and reporting that
            // as truncation would make every complete traversal look incomplete.
            if !context.steps_from(&entity).is_empty() {
                truncated = true;
                truncation_reason.get_or_insert(TruncationReason::MaxDepthReached);
            }
            continue;
        }
        let candidates = context.steps_from(&entity);
        // The ancestors of this entity are the sources of its own steps, plus the start.
        // Reading them off the route rather than keeping a second index is what keeps the
        // cycle check a property of the route a reader is shown.
        let mut ancestors: Vec<&EntityRef> = steps.iter().map(|step| &step.source).collect();
        ancestors.push(&start);

        for step in candidates {
            let target = step.target.clone();
            if ancestors.contains(&&target) {
                // A step back into this route. Recorded as the route's termination
                // rather than followed, because following it would produce a route that
                // revisits an entity and could be shortened.
                cycles_encountered += 1;
                continue;
            }
            if visited.contains(&target) {
                // Reached already by a route with no more steps, which is the shorter
                // one. Reporting this route instead would classify the entity deeper
                // than the graph supports.
                continue;
            }
            if visited.len() >= limits.max_nodes {
                truncated = true;
                truncation_reason.get_or_insert(TruncationReason::MaxNodesReached);
                break;
            }
            let mut route = steps.clone();
            route.push(step);
            let next_depth = depth + 1;
            deepest = deepest.max(next_depth);
            visited.push(target.clone());
            reached.push(Reach {
                entity: target.clone(),
                depth: next_depth,
                steps: route.clone(),
                termination: PathTermination::TargetReached,
            });
            frontier.push_back((target, route, next_depth));
        }
    }

    // Each route's termination is decided once its entity is known: whether it has
    // anywhere further to go, and whether a bound or a cycle stopped it.
    for reach in &mut reached {
        reach.termination = termination_of(context, reach, limits.max_depth);
    }

    reached.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| left.entity.kind.cmp(&right.entity.kind))
            .then_with(|| left.entity.id.cmp(&right.entity.id))
    });

    Propagation {
        start,
        reaches: reached,
        limits,
        boundary: context.boundary.clone(),
        truncated,
        truncation_reason,
        nodes_visited: visited.len(),
        deepest,
        cycles_encountered,
    }
}

/// The findings for a bounded propagation from one entity.
#[must_use]
pub fn findings(
    context: &ImpactContext<'_>,
    changed: &EntityRef,
    limits: Limits,
    change_type: Option<ChangeType>,
) -> Vec<ImpactFinding> {
    propagate(context, changed, limits).findings(change_type)
}

/// Why a route stops where it does.
///
/// Three cases, and the distinction between them is the point. A route ending at the
/// depth bound is [`PathTermination::MaxDepthReached`], so a consumer knows more may lie
/// beyond. A route whose entity has no propagating step left is
/// [`PathTermination::NoPropagatingEdge`], which is the only termination that supports
/// the claim that nothing further exists. Anything else stopped because a cycle or an
/// exclusion cut it short.
fn termination_of(context: &ImpactContext<'_>, reach: &Reach, max_depth: usize) -> PathTermination {
    let outgoing = context.steps_from(&reach.entity);
    if reach.depth >= max_depth && !outgoing.is_empty() {
        return PathTermination::MaxDepthReached;
    }
    if outgoing.is_empty() {
        return PathTermination::NoPropagatingEdge;
    }
    // The entity has somewhere further to go but was already reached by a shorter route,
    // so this route stops here. Reporting that as a cycle would be wrong - the entity is
    // not on this route - and reporting it as complete would be wrong too.
    let on_route: Vec<EntityRef> = reach.nodes();
    if outgoing.iter().any(|step| on_route.contains(&step.target)) {
        return PathTermination::CycleDetected;
    }
    PathTermination::TargetReached
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, Confidence, ConfidenceLevel, DependencyClass, EntityKind, Relationship,
    };
    use amasario_dependency::{Dependency, EvidenceRef};
    use amasario_graph::Graph;

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a non-empty identifier")
    }

    fn edge(
        subject: &EntityRef,
        object: &EntityRef,
        relationship: Relationship,
        level: ConfidenceLevel,
    ) -> Dependency {
        Dependency {
            subject: subject.clone(),
            object: object.clone(),
            relationship,
            classes: vec![DependencyClass::Contract],
            basis: Basis::ObservedInvocation,
            confidence: Confidence::new(level, vec!["a".repeat(64)], Vec::new())
                .expect("a confidence"),
            verification: amasario_core::VerificationStatus::Verified,
            evidence: vec![
                EvidenceRef::new(amasario_core::EvidenceType::Transaction, "a".repeat(64))
                    .expect("a citation"),
            ],
            path: Vec::new(),
            depth: 0,
            reason: "observed".to_owned(),
            observed_at: None,
        }
    }

    fn graph(edges: Vec<Dependency>) -> Graph {
        let mut graph = Graph::new("g").expect("a graph");
        for edge in edges {
            graph.add_edge(edge).expect("an edge");
        }
        graph.canonicalise();
        graph
    }

    fn limits(depth: usize) -> Limits {
        Limits::new(depth, 512).expect("bounded limits")
    }

    /// `A` depended on by `B`, depended on by `C`: a change to `A` reaches `B` then `C`.
    fn chain(levels: &[ConfidenceLevel]) -> Graph {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        graph(vec![
            edge(&b, &a, Relationship::DependsOn, levels[0]),
            edge(&c, &b, Relationship::DependsOn, levels[1]),
        ])
    }

    #[test]
    fn a_two_hop_chain_is_reached_at_the_depth_of_its_shortest_route() {
        let graph = chain(&[ConfidenceLevel::Verified, ConfidenceLevel::Verified]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &entity(EntityKind::Contract, "C-a"), limits(5));
        assert_eq!(propagation.len(), 2);
        assert_eq!(
            propagation.depth_of(&entity(EntityKind::Contract, "C-b")),
            Some(1)
        );
        assert_eq!(
            propagation.depth_of(&entity(EntityKind::Contract, "C-c")),
            Some(2)
        );
        assert!(propagation.is_conclusive());
        let findings = propagation.findings(None);
        assert_eq!(findings.len(), 2);
        let direct = findings
            .iter()
            .find(|finding| finding.hop_depth == 1)
            .expect("a direct finding");
        let transitive = findings
            .iter()
            .find(|finding| finding.hop_depth == 2)
            .expect("a transitive finding");
        assert!(direct.has(crate::affected::ImpactType::Direct));
        assert!(transitive.has(crate::affected::ImpactType::Transitive));
        assert!(!transitive.has(crate::affected::ImpactType::MultiHop));
        assert!(transitive.is_valid(), "{:?}", transitive.failures());
    }

    #[test]
    fn the_aggregate_confidence_of_a_long_route_is_its_weakest_link() {
        let graph = chain(&[ConfidenceLevel::Verified, ConfidenceLevel::LowConfidence]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &entity(EntityKind::Contract, "C-a"), limits(5));
        let findings = propagation.findings(None);
        let transitive = findings
            .iter()
            .find(|finding| finding.hop_depth == 2)
            .expect("a transitive finding");
        assert_eq!(transitive.confidence.level, ConfidenceLevel::LowConfidence);
        assert!(transitive.is_valid(), "{:?}", transitive.failures());
    }

    #[test]
    fn a_route_that_reaches_the_depth_bound_is_not_reported_as_complete() {
        let graph = chain(&[ConfidenceLevel::Verified, ConfidenceLevel::Verified]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &entity(EntityKind::Contract, "C-a"), limits(1));
        assert!(propagation.truncated);
        assert_eq!(
            propagation.truncation_reason,
            Some(TruncationReason::MaxDepthReached)
        );
        assert!(!propagation.is_conclusive());
        let findings = propagation.findings(None);
        assert_eq!(findings.len(), 1);
        let path = findings[0].path.as_ref().expect("a path");
        assert!(!path.is_complete(), "the route stopped at the bound");
        assert_eq!(path.termination, Some(PathTermination::MaxDepthReached));
        assert_eq!(
            findings[0].truncated,
            Some(true),
            "the finding must say the run that produced it was bounded"
        );
    }

    #[test]
    fn a_cycle_is_terminated_rather_than_followed() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        // `A DEPENDS_ON B` and `B DEPENDS_ON A`: a change to A reaches B, and back to A
        // would be the cycle.
        let graph = graph(vec![
            edge(&a, &b, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
        ]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &a, limits(6));
        assert_eq!(
            propagation.len(),
            1,
            "A is not reported as affected by its own change"
        );
        let reach = propagation.reach(&b).expect("B was reached");
        assert_eq!(reach.depth, 1);
        assert_eq!(reach.termination, PathTermination::CycleDetected);
        assert_eq!(propagation.cycles_encountered, 1);
        assert!(propagation.is_conclusive(), "the graph was exhausted");
        // A route that revisits an entity is not producible, so no finding can contain
        // one and the validation rule that refuses it is never the thing that catches it.
        for finding in propagation.findings(None) {
            let nodes = &finding.path.as_ref().expect("a path").nodes;
            let mut seen: Vec<&EntityRef> = Vec::new();
            for node in nodes {
                assert!(!seen.contains(&node), "a route must not revisit an entity");
                seen.push(node);
            }
        }
    }

    #[test]
    fn a_diamond_reaches_the_shared_entity_by_its_shortest_route() {
        // `A <- B`, `A <- C`, `B <- D`, `C <- D`: D is two hops away by either route.
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let d = entity(EntityKind::Contract, "C-d");
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&c, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&d, &b, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&d, &c, Relationship::DependsOn, ConfidenceLevel::Verified),
        ]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &a, limits(6));
        assert_eq!(propagation.len(), 3);
        assert_eq!(propagation.at_depth(2).len(), 1, "D is reached once");
        assert_eq!(propagation.reach(&d).expect("D").depth, 2);
        // Canonical order: by depth, then kind, then identifier.
        let ids: Vec<&str> = propagation
            .entities()
            .into_iter()
            .map(|entity| entity.id.as_str())
            .collect();
        assert_eq!(ids, vec!["C-b", "C-c", "C-d"]);
    }

    #[test]
    fn a_change_that_reaches_nothing_is_a_conclusive_negative_result() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let graph = graph(vec![edge(
            &a,
            &b,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        )]);
        let context = ImpactContext::new(&graph);
        // A change to the dependent reaches nothing upstream, and the graph was fully
        // explored, so this is a finding rather than a gap.
        let propagation = propagate(&context, &a, limits(6));
        assert!(propagation.is_empty());
        assert!(propagation.is_conclusive());
        assert!(propagation.findings(None).is_empty());
    }

    #[test]
    fn the_node_bound_stops_propagation_and_says_so() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&c, &b, Relationship::DependsOn, ConfidenceLevel::Verified),
        ]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &a, Limits::new(6, 2).expect("bounded limits"));
        assert!(
            propagation.truncated,
            "a traversal that stopped at its node bound is not complete"
        );
        assert_eq!(
            propagation.truncation_reason,
            Some(TruncationReason::MaxNodesReached)
        );
        assert!(
            propagation.len() < 2,
            "the third entity must not be reported as reached"
        );
    }

    #[test]
    fn a_disconnected_entity_is_not_reached() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let island = entity(EntityKind::Contract, "C-island");
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(
                &entity(EntityKind::Contract, "C-other"),
                &island,
                Relationship::DependsOn,
                ConfidenceLevel::Verified,
            ),
        ]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &a, limits(6));
        assert!(propagation.reach(&island).is_none());
        assert!(propagation.is_conclusive());
    }

    #[test]
    fn a_three_hop_route_is_classified_as_multi_hop_as_well_as_transitive() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let d = entity(EntityKind::Contract, "C-d");
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&c, &b, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&d, &c, Relationship::DependsOn, ConfidenceLevel::Verified),
        ]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &a, limits(6));
        let findings = propagation.findings(None);
        let deep = findings
            .iter()
            .find(|finding| finding.hop_depth == 3)
            .expect("a three-hop finding");
        assert!(deep.has(crate::affected::ImpactType::Transitive));
        assert!(deep.has(crate::affected::ImpactType::MultiHop));
        assert!(deep.is_multi_hop());
        assert_eq!(
            deep.relationship_types,
            vec![Relationship::DependsOn; 3],
            "the relationship list is in path order"
        );
        assert!(deep.is_valid(), "{:?}", deep.failures());
    }

    #[test]
    fn a_route_that_turns_around_is_reported_as_having_gone_both_ways() {
        // `A DEPENDS_ON B` and `A AFFECTS C`: a change to B reaches A against the arrow,
        // then C with it.
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let graph = graph(vec![
            edge(&a, &b, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&a, &c, Relationship::Affects, ConfidenceLevel::Verified),
        ]);
        let context = ImpactContext::new(&graph);
        let propagation = propagate(&context, &b, limits(4));
        let finding = propagation
            .findings(None)
            .into_iter()
            .find(|finding| finding.affected_entity == c)
            .expect("C was affected");
        assert_eq!(
            finding.direction,
            Some(crate::propagation::ImpactDirection::Both)
        );
    }
}
