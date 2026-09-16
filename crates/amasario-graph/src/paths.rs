//! Path discovery: how one entity reaches another.
//!
//! # Why both the shortest path and all paths
//!
//! They answer different questions and a report needs both. "Is there a route, and what is
//! the one a reader can check most cheaply" is the shortest path, and it is what a
//! dependency explanation quotes. "Are there other routes, and did any of them go somewhere
//! the first one did not" is the enumeration, and it is what stops a report from presenting
//! one route as the only route. `dependency/transitive-dependency` requires that a path be
//! named rather than collapsed; reporting a single route and implying it is the whole story
//! would satisfy the letter of that rule and defeat its purpose.
//!
//! # Why enumeration is bounded on three axes
//!
//! The number of simple paths between two nodes is exponential in the worst case, so an
//! unbounded enumeration is not a slow query - it is a query that does not terminate on
//! inputs this engine exists to handle. Every bound is therefore explicit and reported: the
//! hop limit from [`amasario_dependency::Limits`], the exploration budget from the same
//! structure, and [`DEFAULT_MAX_PATHS`]. Hitting any of them sets `truncated` and a reason,
//! because a partial list of routes that does not say it is partial reads as a complete one.
//!
//! # Why the walk is recursive
//!
//! The depth is bounded by configuration and capped by
//! [`amasario_dependency::transitive::MAX_PERMITTED_DEPTH`], so the recursion is bounded by a value the
//! caller chose rather than by the input. `clippy.toml` records the same justification for
//! the rest of the workspace: at these depths the recursive form of a path walk states the
//! algorithm rather than narrating a worklist.

use std::collections::VecDeque;

use amasario_core::{EntityRef, Relationship, TruncationReason};
use amasario_dependency::Limits;

use crate::edges::Edge;
use crate::graph::Graph;

/// The greatest number of distinct paths an enumeration will report.
///
/// A bound rather than a limit the caller has to think of: a caller asking for every route
/// through a dense graph is usually asking for "the routes", and a number that makes the
/// question terminate is more useful than a wrong promise of completeness.
pub const DEFAULT_MAX_PATHS: usize = 64;

/// One route from a source to a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// The entities on the route, starting at the source and ending at the destination.
    ///
    /// Both ends are included, unlike a [`amasario_dependency::Dependency`] path, which
    /// carries only the intermediates because its subject and target are separate fields.
    /// A route a reader reconstructs from must not omit the two entities it connects.
    pub entities: Vec<EntityRef>,
    /// The edges traversed, in order, each rendered `"{source} -{RELATIONSHIP}-> {target}"`.
    pub edges: Vec<String>,
}

impl Path {
    /// How many edges the route traverses.
    #[must_use]
    pub const fn hops(&self) -> usize {
        self.edges.len()
    }

    /// The entity the route starts at.
    #[must_use]
    pub fn source(&self) -> Option<&EntityRef> {
        self.entities.first()
    }

    /// The entity the route ends at.
    #[must_use]
    pub fn destination(&self) -> Option<&EntityRef> {
        self.entities.last()
    }

    /// The entities strictly between the source and the destination.
    ///
    /// The same list a transitive dependency carries, so a route and the dependency built
    /// from it cannot disagree about what lies between two entities.
    #[must_use]
    pub fn intermediates(&self) -> &[EntityRef] {
        if self.entities.len() <= 2 {
            return &[];
        }
        &self.entities[1..self.entities.len() - 1]
    }

    /// The edges as one line, for a report or an error message.
    #[must_use]
    pub fn render(&self) -> String {
        self.edges.join(" -> ")
    }

    /// Whether the route visits an entity more than once.
    ///
    /// Always false for a route this module produces: a route that revisited an entity
    /// would contain a cycle and could be shortened, so listing it would pad the result
    /// without saying anything new. Exposed because a caller reconstructing a route from
    /// another source may want to ask.
    #[must_use]
    pub fn repeats_an_entity(&self) -> bool {
        let mut seen: Vec<&EntityRef> = Vec::with_capacity(self.entities.len());
        for entity in &self.entities {
            if seen.contains(&entity) {
                return true;
            }
            seen.push(entity);
        }
        false
    }
}

/// What a path enumeration produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSearch {
    /// The routes found, shortest first, then in canonical order.
    pub paths: Vec<Path>,
    /// Whether the search stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why it stopped, when it did.
    pub truncation_reason: Option<TruncationReason>,
    /// How many candidate edges the search expanded.
    pub explored: usize,
    /// The greatest number of hops any reported route traverses.
    pub deepest: usize,
}

impl PathSearch {
    /// Whether any route was found.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// How many routes were found.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.paths.len()
    }

    /// Whether an empty result is inconclusive rather than a negative finding.
    ///
    /// The distinction the whole bounded-search vocabulary exists for: no route found in a
    /// complete search means there is no route, and no route found in a bounded one means
    /// nothing.
    #[must_use]
    pub const fn is_inconclusive(&self) -> bool {
        self.truncated
    }

    /// The shortest route found, if any.
    #[must_use]
    pub fn shortest(&self) -> Option<&Path> {
        self.paths.first()
    }

    /// The only route found, when there is exactly one.
    ///
    /// Returns `None` for both no route and several, because a caller asking for "the
    /// route" and receiving one of two should be told the question was ambiguous rather
    /// than handed an arbitrary answer.
    #[must_use]
    pub fn only(&self) -> Option<&Path> {
        (self.paths.len() == 1).then(|| &self.paths[0])
    }

    /// The relationship every edge of every route uses, when they all agree.
    ///
    /// `None` when the routes disagree, when a route mixes relationships, or when there are
    /// no routes. Describing a mixed chain as one relationship would attribute to the path
    /// a meaning the specification assigns per edge - and `uniform_relationship` in
    /// [`amasario_dependency::transitive`] refuses for the same reason.
    #[must_use]
    pub fn uniform_relationship(&self, graph: &Graph) -> Option<Relationship> {
        let per_route: Vec<Option<Vec<Relationship>>> = self
            .paths
            .iter()
            .map(|path| relationships_of(graph, &path.edges))
            .collect();
        let first_route = per_route.first()?.clone()?;
        let first = *first_route.first()?;
        if !first_route
            .iter()
            .all(|relationship| *relationship == first)
        {
            return None;
        }
        per_route
            .iter()
            .all(|route| {
                route
                    .as_ref()
                    .is_some_and(|relationships| relationships.iter().all(|r| *r == first))
            })
            .then_some(first)
    }
}

/// The relationships of a rendered edge sequence, or `None` if an edge is not in the graph.
fn relationships_of(graph: &Graph, rendered: &[String]) -> Option<Vec<Relationship>> {
    rendered
        .iter()
        .map(|rendered| {
            graph
                .edges
                .iter()
                .find(|edge| &crate::edges::render(edge.dependency()) == rendered)
                .map(Edge::relationship)
        })
        .collect()
}

/// The shortest route from one entity to another, or `None` when there is none.
///
/// Breadth first, so the first route found is the one with the fewest hops. `None` is
/// returned both when no route exists within the bounds and when the search was bounded and
/// stopped early; use [`all_paths`] and check [`PathSearch::is_inconclusive`] when the
/// difference matters, which is why this convenience does not pretend to be one.
///
/// A route from an entity to itself is not a route and is reported as absent. The
/// interesting question about a cycle is which entities lie on it, and [`crate::cycles`]
/// answers that directly.
#[must_use]
pub fn shortest_path(
    graph: &Graph,
    from: &EntityRef,
    to: &EntityRef,
    limits: Limits,
) -> Option<Path> {
    if from == to || limits.max_depth == 0 {
        return None;
    }
    // The discovery log and the parent records are parallel: entry `i` of each describes
    // the entity discovered at step `i`. Pairing them by position rather than by
    // identifier is what lets a route be rebuilt without a map, and there is one entry per
    // entity because a discovered entity is never discovered again.
    let mut discovered: Vec<EntityRef> = Vec::new();
    let mut parent: Vec<(EntityRef, String)> = Vec::new();
    let mut seen: Vec<EntityRef> = vec![from.clone()];
    let mut queue: VecDeque<(EntityRef, usize)> = VecDeque::new();
    queue.push_back((from.clone(), 0));

    while let Some((entity, depth)) = queue.pop_front() {
        if depth >= limits.max_depth {
            continue;
        }
        for edge in graph.edges_from(&entity) {
            let neighbour = edge.target().clone();
            if seen.contains(&neighbour) {
                // Already reached by a route with no more hops than this one, so
                // re-expanding it could only produce a longer route.
                continue;
            }
            if seen.len() >= limits.max_nodes {
                return None;
            }
            seen.push(neighbour.clone());
            discovered.push(neighbour.clone());
            parent.push((entity.clone(), crate::edges::render(edge.dependency())));
            if neighbour == *to {
                return Some(reconstruct(from, to, &discovered, &parent));
            }
            queue.push_back((neighbour, depth + 1));
        }
    }
    None
}

/// Every simple route from one entity to another, within the engine's default path cap.
#[must_use]
pub fn all_paths(graph: &Graph, from: &EntityRef, to: &EntityRef, limits: Limits) -> PathSearch {
    all_paths_bounded(graph, from, to, limits, DEFAULT_MAX_PATHS)
}

/// Every simple route, with an explicit cap on how many are reported.
///
/// Separate from [`all_paths`] so that a caller who needs a wider enumeration can ask for
/// one and still receive a result that reports its own truncation, rather than a caller who
/// cannot and receives an unbounded search.
#[must_use]
pub fn all_paths_bounded(
    graph: &Graph,
    from: &EntityRef,
    to: &EntityRef,
    limits: Limits,
    max_paths: usize,
) -> PathSearch {
    let mut search = PathSearch {
        paths: Vec::new(),
        truncated: false,
        truncation_reason: None,
        explored: 0,
        deepest: 0,
    };
    if from == to {
        return search;
    }
    if max_paths == 0 {
        // A cap of zero is not a small enumeration; it is one that cannot report anything.
        // Saying so is the only honest answer.
        search.truncated = true;
        search.truncation_reason = Some(TruncationReason::MaxNodesReached);
        return search;
    }

    let mut chain: Vec<EntityRef> = vec![from.clone()];
    extend(
        graph,
        from,
        to,
        limits,
        max_paths,
        &mut chain,
        &mut Vec::new(),
        &mut search,
    );

    // Shortest first, then canonically: the route a report quotes is the one a reader can
    // check most cheaply, and the order is the same on every run.
    search.paths.sort_by(|left, right| {
        left.hops()
            .cmp(&right.hops())
            .then_with(|| rendered_entities(left).cmp(&rendered_entities(right)))
    });
    search
}

/// The entity chain as one comparable string, for a deterministic ordering.
fn rendered_entities(path: &Path) -> String {
    path.entities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// Extends a partial route by one edge.
///
/// A recursive helper rather than a worklist, because the depth is the caller's bound and
/// the recursion states the walk directly. `chain` is the route so far and starts at the
/// source, so a neighbour already in it is a repeat - which is what keeps the enumeration to
/// simple routes and also excludes the source without a separate check.
fn extend(
    graph: &Graph,
    current: &EntityRef,
    target: &EntityRef,
    limits: Limits,
    max_paths: usize,
    chain: &mut Vec<EntityRef>,
    edges: &mut Vec<String>,
    search: &mut PathSearch,
) {
    if search.truncated || search.paths.len() >= max_paths {
        return;
    }

    for edge in graph.edges_from(current) {
        // The cap, and any truncation a nested call has just recorded, are re-checked on
        // every edge rather than only on entry to this call. A recursive `extend` can
        // return having pushed the last path the cap allows and having set `truncated`,
        // and this loop would otherwise carry on to the next edge - where an edge that
        // reaches the target directly pushes one path past the cap. Every push site below
        // checks `search.paths.len() >= max_paths`, so the count held there, but not
        // across a nested call returning: that is how a bounded search came to report
        // more paths than its bound.
        if search.truncated || search.paths.len() >= max_paths {
            return;
        }
        let neighbour = edge.target().clone();
        if chain.contains(&neighbour) {
            continue;
        }
        if search.explored >= limits.max_nodes {
            search.truncated = true;
            search
                .truncation_reason
                .get_or_insert(TruncationReason::MaxNodesReached);
            return;
        }
        search.explored += 1;

        chain.push(neighbour.clone());
        edges.push(crate::edges::render(edge.dependency()));

        if neighbour == *target {
            search.deepest = search.deepest.max(edges.len());
            search.paths.push(Path {
                entities: chain.clone(),
                edges: edges.clone(),
            });
            chain.pop();
            edges.pop();
            if search.paths.len() >= max_paths {
                search.truncated = true;
                search
                    .truncation_reason
                    .get_or_insert(TruncationReason::MaxNodesReached);
                return;
            }
            continue;
        }

        if edges.len() >= limits.max_depth {
            // The route cannot be extended further. If anything is reachable beyond it, the
            // enumeration is partial and must say so; if nothing is, it is complete.
            if !graph.edges_from(&neighbour).is_empty() {
                search.truncated = true;
                search
                    .truncation_reason
                    .get_or_insert(TruncationReason::MaxDepthReached);
            }
            chain.pop();
            edges.pop();
            continue;
        }

        extend(
            graph, &neighbour, target, limits, max_paths, chain, edges, search,
        );
        chain.pop();
        edges.pop();
    }
}

/// Rebuilds the route a breadth-first search found.
///
/// `discovered[i]` is the entity reached at step `i` and `parent[i]` records where it was
/// reached from and by which edge, so the two lists are paired by position.
fn reconstruct(
    from: &EntityRef,
    to: &EntityRef,
    discovered: &[EntityRef],
    parent: &[(EntityRef, String)],
) -> Path {
    let mut entities: Vec<EntityRef> = vec![to.clone()];
    let mut edges: Vec<String> = Vec::new();
    let mut current = to.clone();

    while current != *from {
        let Some(index) = discovered.iter().position(|entity| *entity == current) else {
            // Cannot happen: every discovered entity has a parent record, and the walk
            // stops at the source. Breaking rather than panicking keeps a reconstructed
            // route partial instead of taking the process down.
            break;
        };
        let (previous, rendered) = parent[index].clone();
        edges.push(rendered);
        entities.push(previous.clone());
        current = previous;
    }

    entities.reverse();
    edges.reverse();
    Path { entities, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EvidenceType, LedgerSequence, Network, NetworkType, ObservationBoundary,
    };
    use amasario_dependency::{Candidate, EvidenceRef, resolve};

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
            ledger: LedgerSequence::new(10_000).expect("a ledger"),
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

    fn graph_of(candidates: &[Candidate]) -> Graph {
        let subject = entity(EntityKind::Contract, "C-subject");
        let set = resolve(subject, Some(boundary()), candidates, 8).expect("resolves");
        Graph::from_dependencies("g1", &set).expect("a graph")
    }

    fn limits(max_depth: usize, max_nodes: usize) -> Limits {
        Limits::new(max_depth, max_nodes).expect("a bound")
    }

    fn c(id: &str) -> EntityRef {
        entity(EntityKind::Contract, id)
    }

    /// A chain of diamonds: `2^diamonds` distinct routes from `C-0` to `C-<diamonds>`.
    fn diamonds(count: usize) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        for index in 0..count {
            for arm in ["x", "y"] {
                candidates.push(edge(
                    &format!("C-{index}"),
                    &format!("C-{index}-{arm}"),
                    &format!("{:064x}", index * 2 + usize::from(arm == "y")),
                ));
                candidates.push(edge(
                    &format!("C-{index}-{arm}"),
                    &format!("C-{}", index + 1),
                    &format!("{:064x}", index * 2 + 100 + usize::from(arm == "y")),
                ));
            }
        }
        candidates
    }

    #[test]
    fn a_linear_chain_produces_one_route_with_its_intermediates_named() {
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-target", &"c".repeat(64)),
        ]);
        let search = all_paths(&graph, &c("C-subject"), &c("C-target"), limits(6, 100));
        assert_eq!(search.len(), 1);
        assert!(!search.truncated);
        assert!(!search.is_inconclusive());
        let path = search.only().expect("exactly one route");
        assert_eq!(path.hops(), 3);
        assert_eq!(path.source(), Some(&c("C-subject")));
        assert_eq!(path.destination(), Some(&c("C-target")));
        assert_eq!(path.intermediates(), &[c("C-a"), c("C-b")]);
        assert!(!path.repeats_an_entity());
        assert_eq!(
            path.edges,
            vec![
                "CONTRACT:C-subject -INVOCATES-> CONTRACT:C-a".to_owned(),
                "CONTRACT:C-a -INVOCATES-> CONTRACT:C-b".to_owned(),
                "CONTRACT:C-b -INVOCATES-> CONTRACT:C-target".to_owned(),
            ]
        );
        assert_eq!(
            path.render(),
            "CONTRACT:C-subject -INVOCATES-> CONTRACT:C-a -> \
             CONTRACT:C-a -INVOCATES-> CONTRACT:C-b -> \
             CONTRACT:C-b -INVOCATES-> CONTRACT:C-target"
        );
        assert_eq!(search.deepest, 3);
        assert_eq!(
            search.uniform_relationship(&graph),
            Some(Relationship::Invocates)
        );
    }

    #[test]
    fn two_routes_are_both_reported_and_the_shortest_comes_first() {
        let graph = graph_of(&[
            edge("C-subject", "C-long", &"a".repeat(64)),
            edge("C-long", "C-target", &"b".repeat(64)),
            edge("C-subject", "C-target", &"c".repeat(64)),
        ]);
        let search = all_paths(&graph, &c("C-subject"), &c("C-target"), limits(6, 100));
        assert_eq!(search.len(), 2, "got: {:?}", search.paths);
        assert_eq!(search.paths[0].hops(), 1, "the shortest route first");
        assert_eq!(search.paths[1].hops(), 2);
        assert!(
            search.only().is_none(),
            "a caller asking for the route must be told the question was ambiguous"
        );
        assert_eq!(search.shortest().map(Path::hops), Some(1));
        assert_eq!(
            shortest_path(&graph, &c("C-subject"), &c("C-target"), limits(6, 100))
                .map(|path| path.hops()),
            Some(1)
        );
    }

    #[test]
    fn the_shortest_route_is_found_even_when_a_longer_one_exists() {
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-target", &"c".repeat(64)),
            edge("C-subject", "C-target", &"d".repeat(64)),
        ]);
        let shortest = shortest_path(&graph, &c("C-subject"), &c("C-target"), limits(6, 100))
            .expect("a route");
        assert_eq!(shortest.hops(), 1);
        assert!(shortest.intermediates().is_empty());
        assert_eq!(graph.edges_from(&c("C-subject")).len(), 2);
    }

    #[test]
    fn an_unreachable_target_is_reported_as_no_route_without_truncation() {
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-other", "C-elsewhere", &"b".repeat(64)),
        ]);
        let search = all_paths(&graph, &c("C-subject"), &c("C-target"), limits(6, 100));
        assert!(search.is_empty());
        assert!(
            !search.is_inconclusive(),
            "a complete search that found nothing is a negative finding"
        );
        assert!(shortest_path(&graph, &c("C-subject"), &c("C-target"), limits(6, 100)).is_none());
    }

    #[test]
    fn a_bounded_search_that_found_nothing_is_inconclusive() {
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-target", &"c".repeat(64)),
        ]);
        let search = all_paths(&graph, &c("C-subject"), &c("C-target"), limits(2, 100));
        assert!(search.is_empty(), "the route is three hops away");
        assert!(search.is_inconclusive());
        assert_eq!(
            search.truncation_reason,
            Some(TruncationReason::MaxDepthReached)
        );
    }

    #[test]
    fn a_route_to_itself_is_not_a_route() {
        let graph = graph_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        assert!(shortest_path(&graph, &c("C-subject"), &c("C-subject"), limits(6, 100)).is_none());
        let search = all_paths(&graph, &c("C-subject"), &c("C-subject"), limits(6, 100));
        assert!(search.is_empty());
        assert!(
            !search.truncated,
            "a question with no answer is not a truncated search"
        );
    }

    #[test]
    fn enumeration_does_not_report_routes_that_revisit_an_entity() {
        // One genuine route, plus an edge that would close a loop back onto the source.
        // Listing the looped route would pad the result with something that is only a cycle.
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-subject", &"c".repeat(64)),
            edge("C-b", "C-target", &"d".repeat(64)),
        ]);
        let search = all_paths(&graph, &c("C-subject"), &c("C-target"), limits(8, 100));
        assert_eq!(search.len(), 1);
        assert!(search.paths.iter().all(|path| !path.repeats_an_entity()));
        assert_eq!(
            search.paths[0].entities,
            vec![c("C-subject"), c("C-a"), c("C-b"), c("C-target")]
        );
    }

    #[test]
    fn the_path_cap_truncates_and_says_so() {
        // Four diamonds are sixteen distinct routes, so the cap is what makes the question
        // terminate. A partial list that did not say it was partial would read as complete.
        let graph = graph_of(&diamonds(4));
        let uncapped = all_paths_bounded(&graph, &c("C-0"), &c("C-4"), limits(20, 5_000), 64);
        assert_eq!(uncapped.len(), 16, "got: {:?}", uncapped.paths);
        assert!(
            !uncapped.truncated,
            "sixty-four is enough for sixteen routes"
        );
        assert_eq!(
            uncapped.paths[0].hops(),
            8,
            "the shortest route is two hops per diamond"
        );

        let capped = all_paths_bounded(&graph, &c("C-0"), &c("C-4"), limits(20, 5_000), 4);
        assert_eq!(capped.len(), 4);
        assert!(capped.truncated);
        assert_eq!(
            capped.truncation_reason,
            Some(TruncationReason::MaxNodesReached)
        );

        let none = all_paths_bounded(&graph, &c("C-0"), &c("C-4"), limits(20, 5_000), 0);
        assert!(none.is_empty());
        assert!(
            none.truncated,
            "a cap of zero cannot report anything, and must say so"
        );
    }

    /// A cap is a ceiling even when a nested call is what reached it.
    ///
    /// This is the defect `graph-fuzzer` found, reduced to three edges. `C-subject` has
    /// two routes to `C-target`: one through `C-mid`, and one direct. With a cap of one the
    /// search descends into `C-mid`, which pushes the single path the cap allows and sets
    /// `truncated` - and the parent loop then resumed and pushed the *direct* route as
    /// well, reporting two paths against a cap of one.
    ///
    /// Every push site checked the cap; what none of them checked was that a nested call
    /// had already filled it. The intermediate has to be explored before the direct edge
    /// is reached, and `edges_from` returns edges in insertion order, so the declarations
    /// below are what make the shape reachable rather than an accident of the graph.
    #[test]
    fn a_cap_is_a_ceiling_even_when_a_nested_call_reached_it() {
        let graph = graph_of(&[
            edge("C-subject", "C-mid", &"a".repeat(64)),
            edge("C-mid", "C-target", &"b".repeat(64)),
            edge("C-subject", "C-target", &"c".repeat(64)),
        ]);

        let search = all_paths_bounded(&graph, &c("C-subject"), &c("C-target"), limits(6, 100), 1);

        assert_eq!(
            search.paths.len(),
            1,
            "a cap of one reported {} paths: {:?}",
            search.paths.len(),
            search.paths
        );
        assert!(
            search.truncated,
            "a search that stopped at its cap is partial, and must say so"
        );

        // The count is a ceiling for every cap, not only for the one the fuzzer happened
        // to reach: the same shape is enumerated under a range of caps so that a fix which
        // moved the boundary rather than removing it fails here.
        for cap in 1..=3 {
            let search =
                all_paths_bounded(&graph, &c("C-subject"), &c("C-target"), limits(6, 100), cap);
            assert!(
                search.paths.len() <= cap,
                "a cap of {cap} reported {} paths: {:?}",
                search.paths.len(),
                search.paths
            );
        }
    }

    #[test]
    fn the_exploration_budget_truncates_a_dense_graph() {
        let candidates: Vec<Candidate> = (0..30)
            .map(|index| edge("C-subject", &format!("C-{index}"), &format!("{index:064x}")))
            .collect();
        let graph = graph_of(&candidates);
        let budgeted = all_paths_bounded(&graph, &c("C-subject"), &c("C-zebra"), limits(4, 8), 64);
        assert!(budgeted.truncated);
        assert_eq!(budgeted.explored, 8, "the budget is spent, not exceeded");
        assert_eq!(
            budgeted.truncation_reason,
            Some(TruncationReason::MaxNodesReached)
        );
    }

    #[test]
    fn enumeration_is_deterministic_and_independent_of_insertion_order() {
        let forwards = [
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-target", &"b".repeat(64)),
            edge("C-subject", "C-b", &"c".repeat(64)),
            edge("C-b", "C-target", &"d".repeat(64)),
        ];
        let mut backwards = forwards.clone();
        backwards.reverse();
        let first = all_paths(
            &graph_of(&forwards),
            &c("C-subject"),
            &c("C-target"),
            limits(6, 100),
        );
        let second = all_paths(
            &graph_of(&backwards),
            &c("C-subject"),
            &c("C-target"),
            limits(6, 100),
        );
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        for _ in 0..8 {
            assert_eq!(
                all_paths(
                    &graph_of(&forwards),
                    &c("C-subject"),
                    &c("C-target"),
                    limits(6, 100)
                ),
                first
            );
        }
    }

    #[test]
    fn a_route_and_the_transitive_dependency_built_from_it_name_the_same_intermediates() {
        // Two spellings of one fact would be a second place for the fact to be wrong.
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-target", &"c".repeat(64)),
        ]);
        let path = shortest_path(&graph, &c("C-subject"), &c("C-target"), limits(6, 100))
            .expect("a route");
        assert_eq!(path.intermediates(), &[c("C-a"), c("C-b")]);

        let mut set = resolve(
            c("C-subject"),
            Some(boundary()),
            &[
                edge("C-subject", "C-a", &"a".repeat(64)),
                edge("C-a", "C-b", &"b".repeat(64)),
                edge("C-b", "C-target", &"c".repeat(64)),
            ],
            8,
        )
        .expect("resolves");
        amasario_dependency::close_set(&mut set, &graph, Limits::defaults()).expect("closes");
        let transitive = set
            .transitive
            .iter()
            .find(|dependency| dependency.object == c("C-target"))
            .expect("the target is reached transitively");
        assert_eq!(transitive.path, path.intermediates());
    }

    #[test]
    fn a_mixed_route_is_not_described_as_one_relationship() {
        let subject = c("C-a");
        let candidates = [
            Candidate::new(
                subject.clone(),
                c("C-b"),
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
                c("C-b"),
                c("C-c"),
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
        let set = resolve(subject.clone(), Some(boundary()), &candidates, 6).expect("resolves");
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        let search = all_paths(&graph, &subject, &c("C-c"), limits(6, 100));
        assert_eq!(search.len(), 1);
        assert_eq!(
            search.uniform_relationship(&graph),
            None,
            "a mixed chain has no single relationship"
        );

        let uniform = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-target", &"b".repeat(64)),
        ]);
        let search = all_paths(&uniform, &c("C-subject"), &c("C-target"), limits(6, 100));
        assert_eq!(
            search.uniform_relationship(&uniform),
            Some(Relationship::Invocates)
        );
    }

    #[test]
    fn an_empty_search_reports_no_uniform_relationship_rather_than_guessing_one() {
        let graph = graph_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let search = all_paths(&graph, &c("C-subject"), &c("C-absent"), limits(6, 100));
        assert!(search.is_empty());
        assert_eq!(search.uniform_relationship(&graph), None);
        assert_eq!(search.deepest, 0);
    }

    #[test]
    fn a_direct_route_has_no_intermediates() {
        let graph = graph_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let path =
            shortest_path(&graph, &c("C-subject"), &c("C-a"), limits(4, 100)).expect("a route");
        assert_eq!(path.hops(), 1);
        assert!(path.intermediates().is_empty());
        assert_eq!(path.entities, vec![c("C-subject"), c("C-a")]);
        assert_eq!(path.source(), Some(&c("C-subject")));
        assert_eq!(path.destination(), Some(&c("C-a")));
    }

    #[test]
    fn a_direct_route_that_the_node_bound_forbids_is_reported_as_absent() {
        // The node budget is spent before the target is reached, so the answer is `None`
        // rather than a claim that no route exists.
        let graph = graph_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-subject", "C-b", &"b".repeat(64)),
        ]);
        let search = all_paths_bounded(&graph, &c("C-subject"), &c("C-b"), limits(4, 1), 8);
        assert!(search.is_empty());
        assert!(search.is_inconclusive());
    }
}
