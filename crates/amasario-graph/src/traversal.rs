//! Bounded traversal: what an entity reaches, and what reaches it.
//!
//! # Why both directions live here
//!
//! A dependency question ("what does this contract use?") walks the graph forwards, and
//! an impact question ("what would a change to this artifact touch?") walks it
//! backwards. They are the same traversal over the same adjacency, so they share one
//! implementation, one set of bounds and one vocabulary for saying the search stopped.
//! Implementation detail that differs between them is a difference a reader would have to
//! discover twice.
//!
//! # Why the bounds are the dependency layer's
//!
//! [`amasario_dependency::Limits`] is reused rather than restated, so that a graph walk
//! and a closure over the same topology cannot be configured with two different depth
//! bounds - which is exactly how an analysis comes to report a different reachable set
//! depending on which layer asked.
//!
//! # Why truncation is part of the result
//!
//! A traversal that stopped early and one that exhausted the graph both end. Only the
//! result distinguishes them, and a consumer that cannot tell "nothing is there" from
//! "the search stopped" will read the second as the first. That is the failure the
//! specification calls out, so `truncated` and `truncation_reason` are fields of every
//! [`Walk`] rather than something a caller has to remember to ask for.
//!
//! # What a walk records, and what it does not
//!
//! An entry records the entity, its depth, the chain of entities that reached it and the
//! edges actually traversed. When two edges connect the same pair - a contract that both
//! depends on and invokes another - the first in canonical order is the one traversed, and
//! the others remain in the graph: the walk says how it got there, not everything that
//! exists. A report that needs every relationship between two entities asks the graph.

use std::collections::VecDeque;

use amasario_core::{EntityRef, TruncationReason};
use amasario_dependency::{Cycle, Dependency, Limits};

use crate::graph::Graph;

/// Which way a traversal reads the edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    /// Along the edges: from a subject to the entities it depends on.
    Forward,
    /// Against the edges: from an object to the entities that depend on it.
    ///
    /// Deliberately a view of the same edges rather than a second graph. The graph is
    /// directed, and reversing an edge to represent an impact would be a statement about
    /// a relationship nobody observed.
    Reverse,
}

impl Direction {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
        }
    }
}

/// One entity reached by a traversal, with how it was reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// The entity reached. Never the traversal's start, which is at depth zero by
    /// definition and is reported by [`Walk::start`] instead.
    pub entity: EntityRef,
    /// How many edges lie between the start and this entity.
    pub depth: usize,
    /// The entities strictly between the start and this one, in traversal order.
    ///
    /// Empty at depth one. The dependency layer calls the same thing a path, and its
    /// rationale applies unchanged: a chain that does not name its intermediates is
    /// indistinguishable from a guess that collapsed several hops into one.
    pub path: Vec<EntityRef>,
    /// The edges traversed, in order, each rendered `"{source} -{RELATIONSHIP}-> {target}"`.
    pub edges: Vec<String>,
}

impl Reach {
    /// The edges traversed, as the edges they came from.
    ///
    /// Looked up in the graph rather than stored, so that a `Reach` cannot hold a copy of
    /// an edge that disagrees with the graph it came from.
    #[must_use]
    pub fn resolve_edges<'a>(&self, graph: &'a Graph) -> Vec<&'a crate::edges::Edge> {
        self.edges
            .iter()
            .filter_map(|rendered| {
                graph
                    .edges
                    .iter()
                    .find(|edge| &crate::edges::render(edge.dependency()) == rendered)
            })
            .collect()
    }
}

/// What a bounded traversal produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// The entity traversal started from.
    pub start: EntityRef,
    /// Which way it read the edges.
    pub direction: Direction,
    /// The bounds it ran under.
    pub limits: Limits,
    /// What it reached, in canonical order: by depth, then entity kind, then identifier.
    pub entries: Vec<Reach>,
    /// Whether traversal stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why it stopped, when it did.
    pub truncation_reason: Option<TruncationReason>,
    /// How many entities were visited, including the start.
    pub nodes_visited: usize,
    /// The greatest depth any entry reached.
    pub deepest: usize,
    /// The cycles every entity of which lies within the visited set.
    ///
    /// Reported rather than removed, for the reason `dependency/transitive-dependency`
    /// gives: a consumer that never sees a cycle cannot know the graph was not a tree.
    /// Only cycles wholly inside the visited set are listed, because a cycle the walk
    /// never entered is a fact about the graph rather than about this traversal - and
    /// [`crate::cycles::find`] is there for that question.
    pub cycles: Vec<Cycle>,
}

impl Walk {
    /// Whether anything was reached.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a cycle was found inside the visited set.
    #[must_use]
    pub const fn has_cycles(&self) -> bool {
        !self.cycles.is_empty()
    }

    /// Whether traversal stopped early, which makes an empty result inconclusive rather
    /// than a negative finding.
    #[must_use]
    pub const fn is_inconclusive(&self) -> bool {
        self.truncated
    }

    /// The entity reached at a depth, if it was reached.
    #[must_use]
    pub fn find(&self, entity: &EntityRef) -> Option<&Reach> {
        self.entries.iter().find(|entry| entry.entity == *entity)
    }

    /// The depth an entity was reached at, if it was reached.
    #[must_use]
    pub fn depth_of(&self, entity: &EntityRef) -> Option<usize> {
        self.find(entity).map(|entry| entry.depth)
    }

    /// Every entity reached, in canonical order.
    #[must_use]
    pub fn entities(&self) -> Vec<&EntityRef> {
        self.entries.iter().map(|entry| &entry.entity).collect()
    }

    /// The entries at one depth.
    #[must_use]
    pub fn at_depth(&self, depth: usize) -> Vec<&Reach> {
        self.entries
            .iter()
            .filter(|entry| entry.depth == depth)
            .collect()
    }

    /// The entity identifiers reached, for an error message or a report.
    #[must_use]
    pub fn entity_ids(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.entity.id.as_str())
            .collect()
    }
}

/// Traverses from an entity along the edges, within the given bounds.
#[must_use]
pub fn walk(graph: &Graph, start: &EntityRef, limits: Limits) -> Walk {
    traverse(graph, start, limits, Direction::Forward)
}

/// Traverses from an entity against the edges, within the given bounds.
///
/// The reverse view answers the impact question: which entities depend on this one. Each
/// entry's path is the chain of entities that reach it, so an impact report can name the
/// route rather than only the destination.
#[must_use]
pub fn walk_reverse(graph: &Graph, start: &EntityRef, limits: Limits) -> Walk {
    traverse(graph, start, limits, Direction::Reverse)
}

/// Whether one entity reaches another within the bounds.
///
/// Returns the depth it was reached at, or `None`. `None` is not a negative answer when
/// the search was bounded: use [`walk`] and check [`Walk::is_inconclusive`] when the
/// difference matters, which is the reason this convenience does not pretend to be one.
#[must_use]
pub fn reachable(graph: &Graph, from: &EntityRef, to: &EntityRef, limits: Limits) -> Option<usize> {
    walk(graph, from, limits).depth_of(to)
}

/// The traversal both directions share.
///
/// Breadth first, so the first time an entity is reached is the shortest route to it,
/// and the reported path is the one a reader can check most cheaply. A node already
/// visited is not expanded again: its own edges were reached at a depth less than or
/// equal to the one it would be traversed at now, so re-expanding it could only produce
/// longer paths.
fn traverse(graph: &Graph, start: &EntityRef, limits: Limits, direction: Direction) -> Walk {
    let mut entries: Vec<Reach> = Vec::new();
    let mut visited: Vec<EntityRef> = vec![start.clone()];
    let mut queue: VecDeque<(EntityRef, Vec<EntityRef>, Vec<String>)> = VecDeque::new();
    queue.push_back((start.clone(), Vec::new(), Vec::new()));

    let mut truncated = false;
    let mut truncation_reason: Option<TruncationReason> = None;
    let mut deepest = 0;

    while let Some((entity, path, edges)) = queue.pop_front() {
        let hops = edges_from(graph, &entity, direction);

        // The depth bound is applied before the node bound, and both are reported even
        // when a node with no outgoing edges is sitting at the bound: a node at the bound
        // with nothing beyond it missed nothing, so it is not a truncation.
        if path.len() >= limits.max_depth && !hops.is_empty() {
            truncated = true;
            truncation_reason.get_or_insert(TruncationReason::MaxDepthReached);
            continue;
        }
        if visited.len() >= limits.max_nodes && !hops.is_empty() {
            truncated = true;
            truncation_reason.get_or_insert(TruncationReason::MaxNodesReached);
            continue;
        }

        for edge in hops {
            let neighbour = neighbour_of(edge, direction);
            if neighbour == *start || path.contains(&neighbour) {
                // A cycle, reported by `cycles` rather than followed: following it would
                // revisit entities the walk has already reported.
                continue;
            }
            if visited.contains(&neighbour) {
                continue;
            }
            // Enforced here, per entity, and not only when a node is dequeued: on dequeue
            // alone one expansion can push the visited count past the configured maximum,
            // so a walk would report having visited more entities than the bound it names.
            if visited.len() >= limits.max_nodes {
                truncated = true;
                truncation_reason.get_or_insert(TruncationReason::MaxNodesReached);
                break;
            }

            let mut next_path = path.clone();
            next_path.push(neighbour.clone());
            let mut next_edges = edges.clone();
            next_edges.push(crate::edges::render(edge));
            let depth = next_edges.len();
            deepest = deepest.max(depth);
            visited.push(neighbour.clone());
            entries.push(Reach {
                entity: neighbour.clone(),
                depth,
                path: path.clone(),
                edges: next_edges.clone(),
            });
            queue.push_back((neighbour, next_path, next_edges));
        }
    }

    entries.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| left.entity.kind.as_str().cmp(right.entity.kind.as_str()))
            .then_with(|| left.entity.id.cmp(&right.entity.id))
    });

    let visited_ids: Vec<String> = visited.iter().map(ToString::to_string).collect();
    let cycles: Vec<Cycle> = crate::cycles::find(graph)
        .into_iter()
        .filter(|cycle| {
            cycle
                .entities
                .iter()
                .all(|entity| visited_ids.contains(&entity.to_string()))
        })
        .collect();

    Walk {
        start: start.clone(),
        direction,
        limits,
        entries,
        truncated,
        truncation_reason,
        nodes_visited: visited.len(),
        deepest,
        cycles,
    }
}

/// The edges to follow from an entity, in the direction being traversed.
///
/// Forward follows the edges leaving the entity; reverse follows the edges arriving at
/// it. Both are ordered by the neighbour's canonical position, so two runs over the same
/// graph visit neighbours in the same order and report the same paths.
fn edges_from<'a>(
    graph: &'a Graph,
    entity: &EntityRef,
    direction: Direction,
) -> Vec<&'a Dependency> {
    let mut dependencies: Vec<&Dependency> = graph
        .edges
        .iter()
        .filter(|edge| match direction {
            Direction::Forward => edge.source() == entity,
            Direction::Reverse => edge.target() == entity,
        })
        .map(crate::edges::Edge::dependency)
        .collect();
    dependencies.sort_by(|left, right| {
        let left_neighbour = neighbour_of(left, direction);
        let right_neighbour = neighbour_of(right, direction);
        left_neighbour
            .kind
            .as_str()
            .cmp(right_neighbour.kind.as_str())
            .then_with(|| left_neighbour.id.cmp(&right_neighbour.id))
            .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
    });
    dependencies
}

/// The entity an edge leads to, in the direction being traversed.
fn neighbour_of(edge: &Dependency, direction: Direction) -> EntityRef {
    match direction {
        Direction::Forward => edge.object.clone(),
        Direction::Reverse => edge.subject.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EvidenceType, LedgerSequence, Network, NetworkType, ObservationBoundary,
        Relationship,
    };
    use amasario_dependency::{Candidate, DependencySet, EvidenceRef, close, resolve};

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
            ledger: LedgerSequence::new(7_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn edge(from: &str, to: &str, transaction: &str) -> Candidate {
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

    fn graph_of(candidates: &[Candidate]) -> (Graph, DependencySet) {
        let subject = entity(EntityKind::Contract, "C-subject");
        let set = resolve(subject, Some(boundary()), candidates, 8).expect("resolves");
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        (graph, set)
    }

    fn limits(max_depth: usize, max_nodes: usize) -> Limits {
        Limits::new(max_depth, max_nodes).expect("a bound")
    }

    #[test]
    fn a_forward_walk_records_depth_path_and_the_edges_it_traversed() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-c", &"c".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let found = walk(&graph, &start, limits(5, 100));

        assert_eq!(
            found.depth_of(&entity(EntityKind::Contract, "C-a")),
            Some(1)
        );
        assert_eq!(
            found.depth_of(&entity(EntityKind::Contract, "C-c")),
            Some(3)
        );
        let deepest = found
            .find(&entity(EntityKind::Contract, "C-c"))
            .expect("reached");
        assert_eq!(
            deepest.path,
            vec![
                entity(EntityKind::Contract, "C-a"),
                entity(EntityKind::Contract, "C-b")
            ],
            "the intermediates are named, or the chain is indistinguishable from a guess"
        );
        assert_eq!(deepest.edges.len(), 3);
        assert_eq!(
            deepest.edges,
            vec![
                "CONTRACT:C-subject -INVOCATES-> CONTRACT:C-a".to_owned(),
                "CONTRACT:C-a -INVOCATES-> CONTRACT:C-b".to_owned(),
                "CONTRACT:C-b -INVOCATES-> CONTRACT:C-c".to_owned(),
            ]
        );
        assert_eq!(deepest.resolve_edges(&graph).len(), 3);
        assert_eq!(found.nodes_visited, 4);
        assert_eq!(found.deepest, 3);
        assert!(!found.truncated);
        assert!(!found.is_inconclusive());
        assert_eq!(found.direction, Direction::Forward);
    }

    #[test]
    fn a_reverse_walk_answers_the_impact_question_over_the_same_edges() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-downstream", &"c".repeat(64)),
        ]);
        let changed = entity(EntityKind::Contract, "C-downstream");
        let dependents = walk_reverse(&graph, &changed, limits(5, 100));

        // `C-b` invoked the changed contract, and `C-a` reached it through `C-b`, and the
        // subject through both - so every one of them is exposed.
        assert_eq!(
            dependents.depth_of(&entity(EntityKind::Contract, "C-b")),
            Some(1)
        );
        assert_eq!(
            dependents.depth_of(&entity(EntityKind::Contract, "C-a")),
            Some(2)
        );
        assert_eq!(
            dependents.depth_of(&entity(EntityKind::Contract, "C-subject")),
            Some(3)
        );
        assert_eq!(dependents.direction, Direction::Reverse);

        // The forward walk from the same start reaches nothing, which is the distinction
        // that makes the two directions worth having separately.
        assert!(walk(&graph, &changed, limits(5, 100)).is_empty());
    }

    #[test]
    fn the_walk_and_the_dependency_closure_agree_about_what_is_reachable() {
        // The conformance that keeps the graph layer's own traversal honest: if these
        // disagreed, a dependency report and an impact report would disagree about the
        // same topology.
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-c", &"c".repeat(64)),
            edge("C-a", "C-d", &"d".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let closure = close(&start, &graph, Limits::defaults());
        let found = walk(&graph, &start, Limits::defaults());

        // The closure returns only the *transitive* partition: an entity one edge from the
        // subject belongs to the direct partition, and reporting it here as well would put
        // one edge in both. The walk reports everything it reaches, so the comparison is
        // against the walk's entries beyond depth one.
        let mut from_closure: Vec<String> = closure
            .entries
            .iter()
            .map(|entry| entry.object.to_string())
            .collect();
        let mut from_walk: Vec<String> = found
            .entries
            .iter()
            .filter(|entry| entry.depth >= 2)
            .map(|entry| entry.entity.to_string())
            .collect();
        from_closure.sort();
        from_walk.sort();
        assert_eq!(from_closure, from_walk);
        assert_eq!(
            found.depth_of(&entity(EntityKind::Contract, "C-a")),
            Some(1),
            "and the walk does reach the direct neighbour, which the closure omits by design"
        );
        assert_eq!(closure.truncated, found.truncated);
        assert!(!closure.truncated);
    }

    #[test]
    fn a_depth_bound_is_reported_rather_than_producing_a_short_answer() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-c", &"c".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let bounded = walk(&graph, &start, limits(2, 100));
        assert_eq!(bounded.deepest, 2);
        assert!(bounded.truncated);
        assert_eq!(
            bounded.truncation_reason,
            Some(TruncationReason::MaxDepthReached)
        );
        assert!(
            bounded.is_inconclusive(),
            "an empty result would be inconclusive"
        );
        assert!(
            bounded
                .depth_of(&entity(EntityKind::Contract, "C-c"))
                .is_none()
        );

        // A node at the bound with nothing beyond it missed nothing, so the same bound
        // over a graph that ends there is a complete answer.
        let (shallow, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
        ]);
        let complete = walk(&shallow, &start, limits(2, 100));
        assert!(!complete.truncated);
        assert_eq!(complete.deepest, 2);
    }

    #[test]
    fn a_node_bound_is_reported_with_its_own_reason() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-subject", "C-b", &"b".repeat(64)),
            edge("C-subject", "C-c", &"c".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let bounded = walk(&graph, &start, limits(10, 3));
        assert!(bounded.truncated);
        assert_eq!(
            bounded.truncation_reason,
            Some(TruncationReason::MaxNodesReached),
            "the node bound, not the depth bound, is what stopped this"
        );
        assert_eq!(bounded.nodes_visited, 3);
    }

    #[test]
    fn the_start_is_never_reported_as_reached_by_itself() {
        let (graph, _) = graph_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let start = entity(EntityKind::Contract, "C-subject");
        let found = walk(&graph, &start, limits(5, 100));
        assert!(found.depth_of(&start).is_none());
        assert_eq!(found.entity_ids(), vec!["C-a"]);
    }

    #[test]
    fn a_cycle_is_not_followed_and_is_reported_inside_the_visited_set() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-a", &"c".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let found = walk(&graph, &start, limits(10, 100));
        assert_eq!(
            found.depth_of(&entity(EntityKind::Contract, "C-a")),
            Some(1)
        );
        assert_eq!(
            found.depth_of(&entity(EntityKind::Contract, "C-b")),
            Some(2)
        );
        assert!(
            found
                .depth_of(&entity(EntityKind::Contract, "C-a"))
                .unwrap()
                < 4,
            "the cycle must not be walked repeatedly"
        );
        assert!(found.has_cycles());
        assert_eq!(found.cycles.len(), 1);
        assert!(!found.truncated, "a cycle is not a truncated search");
    }

    #[test]
    fn a_cycle_outside_the_visited_set_is_not_reported_against_this_walk() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-far", "C-far2", &"b".repeat(64)),
            edge("C-far2", "C-far", &"c".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let found = walk(&graph, &start, limits(10, 100));
        assert!(
            !found.has_cycles(),
            "a cycle the walk never entered is a fact about the graph, not about this traversal"
        );
        assert_eq!(
            crate::cycles::find(&graph).len(),
            1,
            "and it is still in the graph"
        );
    }

    #[test]
    fn the_shortest_route_wins_and_the_result_is_deterministic() {
        // Two routes to the same entity: the breadth-first walk keeps the shorter one,
        // which is the one a reader can verify most cheaply.
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-long", &"a".repeat(64)),
            edge("C-long", "C-target", &"b".repeat(64)),
            edge("C-subject", "C-target", &"c".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let target = entity(EntityKind::Contract, "C-target");
        let first = walk(&graph, &start, limits(10, 100));
        assert_eq!(first.depth_of(&target), Some(1));
        assert!(first.find(&target).expect("reached").path.is_empty());

        for _ in 0..8 {
            let again = walk(&graph, &start, limits(10, 100));
            assert_eq!(again, first);
        }
    }

    #[test]
    fn entries_are_ordered_by_depth_then_entity() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-zeta", &"1".repeat(64)),
            edge("C-subject", "C-alpha", &"2".repeat(64)),
            edge("C-alpha", "C-deep", &"3".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let found = walk(&graph, &start, limits(10, 100));
        assert_eq!(
            found.entity_ids(),
            vec!["C-alpha", "C-zeta", "C-deep"],
            "depth first, then identifier"
        );
        assert_eq!(found.at_depth(1).len(), 2);
        assert_eq!(found.at_depth(2).len(), 1);
    }

    #[test]
    fn reachability_is_answered_with_the_depth_it_was_reached_at() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        assert_eq!(
            reachable(
                &graph,
                &start,
                &entity(EntityKind::Contract, "C-b"),
                limits(5, 100)
            ),
            Some(2)
        );
        assert_eq!(
            reachable(
                &graph,
                &start,
                &entity(EntityKind::Contract, "C-nope"),
                limits(5, 100)
            ),
            None
        );
    }

    #[test]
    fn a_zero_bound_is_refused_rather_than_producing_silence() {
        // The bound comes from the dependency layer, which refuses a zero for exactly
        // this reason: a traversal that visits nothing would report an empty result as
        // though it were a complete one.
        assert!(Limits::new(0, 10).is_err());
        assert!(Limits::new(10, 0).is_err());
    }

    #[test]
    fn every_edge_between_a_pair_stays_in_the_graph_even_when_only_one_is_walked() {
        // A contract that both depends on and invokes another: the walk records the
        // relationship it traversed, and the other is still there for a report to list.
        let subject = entity(EntityKind::Contract, "C-a");
        let other = entity(EntityKind::Contract, "C-b");
        let candidates = [
            Candidate::new(
                subject.clone(),
                other.clone(),
                Relationship::Invocates,
                Basis::ObservedInvocation,
                vec![
                    EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64))
                        .expect("a citation"),
                ],
            )
            .expect("a candidate")
            .observed_at(boundary())
            .with_outcome(Some(true)),
            Candidate::new(
                subject.clone(),
                other.clone(),
                Relationship::DependsOn,
                Basis::ObservedInvocation,
                vec![
                    EvidenceRef::new(EvidenceType::Transaction, "b".repeat(64))
                        .expect("a citation"),
                ],
            )
            .expect("a candidate")
            .observed_at(boundary())
            .with_outcome(Some(true)),
        ];
        let set = resolve(subject.clone(), Some(boundary()), &candidates, 4).expect("resolves");
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        assert_eq!(graph.edges_between(&subject, &other).len(), 2);

        let found = walk(&graph, &subject, limits(4, 10));
        assert_eq!(
            found.entries.len(),
            1,
            "one entity, however many relationships"
        );
        assert_eq!(
            found.entries[0].edges.len(),
            1,
            "the edge actually traversed"
        );
    }

    #[test]
    fn the_walks_order_matches_the_graphs_adjacency_order() {
        let (graph, _) = graph_of(&[
            edge("C-subject", "C-zeta", &"1".repeat(64)),
            edge("C-subject", "C-alpha", &"2".repeat(64)),
        ]);
        let start = entity(EntityKind::Contract, "C-subject");
        let from_source: Vec<String> = graph
            .edges_from(&start)
            .into_iter()
            .map(|edge| edge.target().to_string())
            .collect();
        let from_walk: Vec<String> = walk(&graph, &start, limits(4, 10))
            .entities()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(from_source, from_walk);
    }
}
