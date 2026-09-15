//! The transitive partition: what the subject reaches through intermediates.
//!
//! # The rule this module implements
//!
//! `dependency/transitive-dependency` is unusually specific, and every clause of it is
//! a requirement this module meets rather than a design choice it makes.
//!
//! * A `TRANSITIVE` dependency **MUST carry the intermediate entities** between its
//!   source and target, in order, with at least one entry. Without them it is
//!   indistinguishable from a guess: the reader cannot tell whether the analysis walked
//!   a real chain or collapsed two uncertain hops into one confident-looking
//!   statement.
//! * Its **aggregated confidence MUST equal the minimum** among the hops it traverses.
//!   A chain is only as strong as its weakest link, and taking the strongest hop would
//!   let confidence be manufactured from unrelated evidence.
//! * Traversal **MUST set its depth bound**, and the result **MUST say it was
//!   truncated** with a reason when it stopped early. A bounded search that does not
//!   say it was bounded invites the consumer to conclude that a dependency does not
//!   exist, when the analysis merely stopped.
//! * Any **cycle MUST be reported** with the edges that form it rather than removed or
//!   collapsed. A consumer that never sees the cycle cannot know the graph was not a
//!   tree.
//!
//! # Why the shortest path wins
//!
//! When the subject reaches an entity by two routes, the first one found is kept. The
//! order is deterministic - breadth first, with each node's edges ordered by the same
//! rule the resolver uses - so two runs over the same evidence agree, and the path
//! reported is the one with the fewest hops, which is the one a reader can verify most
//! cheaply. Alternative paths are not discarded from the evidence: their citations
//! travel on the kept edge, because the dependency is still supported by all of them.

use std::collections::VecDeque;

use amasario_core::{
    Confidence, ConfidenceLevel, DependencyClass, EntityRef, Relationship, Result,
    TruncationReason, VerificationStatus,
};
use serde::{Deserialize, Serialize};

use crate::resolver::{Cycle, Dependency, DependencySet, weakest_status};

/// The depth bound traversal is given by default.
///
/// Re-exported from the engine's configuration rather than defined here, so that a
/// caller cannot end up with two different defaults for the same bound.
pub const DEFAULT_MAX_DEPTH: usize = amasario_core::DEFAULT_MAX_DEPTH as usize;

/// The node bound traversal is given by default.
pub const DEFAULT_MAX_NODES: usize = amasario_core::DEFAULT_MAX_NODES;

/// The greatest depth bound the engine will honour, whatever the configuration says.
///
/// A configured depth beyond this is clamped rather than refused: the analysis still
/// runs and still reports its result, and the bound it actually used is the one
/// recorded in [`Limits`], so a consumer can see what happened.
pub const MAX_PERMITTED_DEPTH: usize = amasario_core::MAX_PERMITTED_DEPTH as usize;

/// The bounds a traversal runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    /// The greatest number of edges a reported path may traverse.
    pub max_depth: usize,
    /// The greatest number of entities traversal may visit.
    pub max_nodes: usize,
}

impl Limits {
    /// Builds a set of bounds.
    ///
    /// # Errors
    ///
    /// Returns a dependency error when either bound is zero. A zero bound is not a
    /// small traversal; it is a traversal that cannot report anything, and accepting it
    /// would let an analysis be configured into producing silence.
    pub fn new(max_depth: usize, max_nodes: usize) -> Result<Self> {
        if max_depth == 0 || max_nodes == 0 {
            return Err(amasario_core::EngineError::Dependency(
                "a traversal bound of zero would visit nothing and report an empty result as \
                 though it were a complete one"
                    .to_owned(),
            ));
        }
        Ok(Self {
            max_depth: max_depth.min(MAX_PERMITTED_DEPTH),
            max_nodes,
        })
    }

    /// The engine's defaults.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_nodes: DEFAULT_MAX_NODES,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Where the edges of a graph come from.
///
/// A trait rather than a concrete graph so that the closure can run over a resolved
/// set, over a flat edge list, or over a caller's own lookup, without this crate
/// depending on the graph crate - which is the layer above, and would otherwise have to
/// be depended upon from below.
pub trait EdgeSource {
    /// The edges leaving an entity, in a deterministic order.
    fn edges_from(&self, entity: &EntityRef) -> Vec<Dependency>;
}

impl EdgeSource for DependencySet {
    fn edges_from(&self, entity: &EntityRef) -> Vec<Dependency> {
        // Only the direct partition: a transitive entry describes a whole path, and
        // traversing those would report a path through a path.
        self.direct
            .iter()
            .filter(|edge| edge.subject == *entity)
            .cloned()
            .collect()
    }
}

impl EdgeSource for [Dependency] {
    fn edges_from(&self, entity: &EntityRef) -> Vec<Dependency> {
        let mut edges: Vec<Dependency> = self
            .iter()
            .filter(|edge| edge.subject == *entity && edge.depth == 0)
            .cloned()
            .collect();
        edges.sort_by(|left, right| {
            left.object
                .kind
                .as_str()
                .cmp(right.object.kind.as_str())
                .then_with(|| left.object.id.cmp(&right.object.id))
                .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
        });
        edges
    }
}

impl EdgeSource for Vec<Dependency> {
    fn edges_from(&self, entity: &EntityRef) -> Vec<Dependency> {
        self.as_slice().edges_from(entity)
    }
}

impl<F> EdgeSource for F
where
    F: Fn(&EntityRef) -> Vec<Dependency>,
{
    fn edges_from(&self, entity: &EntityRef) -> Vec<Dependency> {
        self(entity)
    }
}

/// What closing a set produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closure {
    /// The entity traversal started from.
    pub subject: EntityRef,
    /// The bounds it ran under.
    pub limits: Limits,
    /// The dependencies reached, each carrying its path.
    pub entries: Vec<Dependency>,
    /// The cycles found, with the edges that form them.
    pub cycles: Vec<Cycle>,
    /// Whether traversal stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why it stopped, when it did.
    pub truncation_reason: Option<TruncationReason>,
    /// How many entities were visited.
    pub nodes_visited: usize,
    /// The greatest depth any reported dependency reached.
    pub deepest: usize,
}

impl Closure {
    /// Whether any dependency was found.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a cycle was found.
    #[must_use]
    pub const fn has_cycles(&self) -> bool {
        !self.cycles.is_empty()
    }
}

/// A node waiting to be expanded, with the path that reached it.
struct Frontier {
    entity: EntityRef,
    /// The entities on the path from the subject to this one, ending here.
    ///
    /// Empty for the subject itself, which is what makes the length of this vector the
    /// depth of the node and the length of any path that continues through it.
    path: Vec<EntityRef>,
    /// The edges traversed to reach this node, in order.
    chain: Vec<Dependency>,
}

/// Closes a set under a bounded breadth-first traversal.
///
/// The depth bound is applied before the node bound, so that a result truncated by
/// depth says `MaxDepthReached` even if it also visited many nodes: the depth bound is
/// the one the caller asked for, and it is the more informative of the two.
///
/// **Depth-one targets are traversed but not reported.** A [`DependencyClass::Transitive`]
/// dependency is one the subject reaches *only* through intermediates, so the entities
/// at depth one belong to the direct partition and reporting them here as well would
/// put one edge in both partitions - which `dependency/transitive-dependency` forbids
/// and [`crate::resolver::DependencySet::validate`] refuses.
#[must_use]
pub fn close(subject: &EntityRef, source: &dyn EdgeSource, limits: Limits) -> Closure {
    let mut queue: VecDeque<Frontier> = VecDeque::new();
    queue.push_back(Frontier {
        entity: subject.clone(),
        path: Vec::new(),
        chain: Vec::new(),
    });

    let mut visited: Vec<EntityRef> = vec![subject.clone()];
    let mut entries = Vec::new();
    let mut cycles = Vec::new();
    let mut truncated = false;
    let mut truncation_reason = None;
    let mut deepest = 0;

    while let Some(frontier) = queue.pop_front() {
        let depth = frontier.path.len();
        let edges = source.edges_from(&frontier.entity);

        if depth >= limits.max_depth && !edges.is_empty() {
            // Something reachable was left unvisited, so the result is partial and must
            // say so. A node with no edges at the bound missed nothing.
            truncated = true;
            truncation_reason.get_or_insert(TruncationReason::MaxDepthReached);
            continue;
        }
        if visited.len() >= limits.max_nodes && !edges.is_empty() {
            truncated = true;
            truncation_reason.get_or_insert(TruncationReason::MaxNodesReached);
            continue;
        }

        for edge in edges {
            let target = edge.object.clone();

            // A target already on the path, or the subject, closes a cycle. It is
            // reported rather than collapsed: a consumer that never sees it cannot know
            // the graph was not a tree.
            if target == *subject || frontier.path.contains(&target) || target == frontier.entity {
                cycles.push(cycle_for(subject, &frontier, &edge));
                continue;
            }
            if visited.contains(&target) {
                // Already reached by a shorter or equal path, which breadth-first
                // traversal guarantees was found first. The edge is not lost: its
                // citations travel on the kept entry.
                continue;
            }
            // The bound is enforced here, per entity, rather than only when a node is
            // dequeued. Enforcing it on dequeue alone lets a single expansion push the
            // visited count past the configured maximum, so a result could report having
            // visited more entities than the bound it names - which makes the bound a
            // suggestion rather than a bound. The check sits after the cycle check on
            // purpose: reporting a cycle is not visiting a node, and refusing to report
            // one because the budget ran out would hide real topology.
            if visited.len() >= limits.max_nodes {
                truncated = true;
                truncation_reason.get_or_insert(TruncationReason::MaxNodesReached);
                break;
            }

            let mut chain = frontier.chain.clone();
            chain.push(edge);
            // The intermediates are the nodes strictly between the subject and the
            // target, which is the path of the node the edge leaves from.
            let path = frontier.path.clone();
            let entry = transitive_dependency(subject, &target, path, &chain);
            let reached_depth = entry.depth;
            deepest = deepest.max(reached_depth);
            if reached_depth >= 2 {
                entries.push(entry);
            }
            visited.push(target.clone());

            let mut next_path = frontier.path.clone();
            next_path.push(target.clone());
            queue.push_back(Frontier {
                entity: target,
                path: next_path,
                chain,
            });
        }
    }

    entries.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| left.object.kind.as_str().cmp(right.object.kind.as_str()))
            .then_with(|| left.object.id.cmp(&right.object.id))
    });
    cycles.sort_by(|left, right| left.entities[0].id.cmp(&right.entities[0].id));

    Closure {
        subject: subject.clone(),
        limits,
        entries,
        cycles,
        truncated,
        truncation_reason,
        nodes_visited: visited.len(),
        deepest,
    }
}

/// Builds the cycle a re-entry closes.
fn cycle_for(subject: &EntityRef, frontier: &Frontier, edge: &Dependency) -> Cycle {
    // The frontier's path already ends at the node the edge leaves from, so appending
    // only the target closes the loop exactly once.
    let mut entities = vec![subject.clone()];
    entities.extend(frontier.path.iter().cloned());
    entities.push(edge.object.clone());
    let mut edges: Vec<String> = frontier.chain.iter().map(edge_identifier).collect();
    edges.push(edge_identifier(edge));
    Cycle { entities, edges }
}

fn edge_identifier(dependency: &Dependency) -> String {
    format!(
        "{} -{}-> {}",
        dependency.subject,
        dependency.relationship.as_str(),
        dependency.object
    )
}

/// Builds a transitive dependency from the chain that reached it.
fn transitive_dependency(
    subject: &EntityRef,
    target: &EntityRef,
    path: Vec<EntityRef>,
    chain: &[Dependency],
) -> Dependency {
    let depth = chain.len();
    let last = chain.last().expect("a chain always has a final edge");

    // The aggregation the rule requires: the minimum across the hops.
    let level = chain
        .iter()
        .map(|hop| hop.confidence.level)
        .fold(ConfidenceLevel::Verified, ConfidenceLevel::weakest);
    let verification = chain
        .iter()
        .map(|hop| hop.verification)
        .fold(VerificationStatus::Verified, |left, right| {
            weakest_status(left, right)
        });

    let mut evidence = Vec::new();
    for hop in chain {
        for citation in &hop.evidence {
            if !evidence.contains(citation) {
                evidence.push(citation.clone());
            }
        }
    }
    let citations: Vec<String> = evidence.iter().map(ToString::to_string).collect();
    let confidence = Confidence::new(level, citations, Vec::new())
        .unwrap_or_else(|_| last.confidence.clone())
        .with_rationale(format!(
            "the weakest of {depth} hops sets a ceiling of {level}"
        ));

    let mut classes = last.classes.clone();
    if !classes.contains(&DependencyClass::Transitive) {
        classes.push(DependencyClass::Transitive);
        classes.sort_by_key(|class| {
            DependencyClass::all()
                .iter()
                .position(|known| known == class)
                .unwrap_or(usize::MAX)
        });
    }

    let trace = chain
        .iter()
        .map(|hop| format!("{} {}", hop.relationship, hop.object))
        .collect::<Vec<_>>()
        .join(" -> ");

    Dependency {
        subject: subject.clone(),
        object: target.clone(),
        relationship: last.relationship,
        classes,
        basis: last.basis,
        confidence,
        verification,
        evidence,
        path,
        depth,
        reason: format!("reached in {depth} hop(s): {subject} -> {trace}"),
        observed_at: chain
            .iter()
            .filter_map(|hop| hop.observed_at.clone())
            .next(),
    }
}

/// Merges a closure into a set, partitioning the edges it found.
///
/// An edge already in the direct partition is dropped from the closure, because
/// `dependency/transitive-dependency` requires every edge to appear in exactly one of
/// the two partitions.
///
/// # Errors
///
/// Returns a dependency error when the merged set violates an invariant.
pub fn apply(set: &mut DependencySet, closure: Closure) -> Result<()> {
    for entry in closure.entries {
        // The subject is compared as well, because "the object is already in the direct
        // partition" is not the same statement as "the subject already reaches it
        // directly". Observations are resolved together, so a set can hold an edge whose
        // subject is an intermediate - and reading that as the subject's own direct edge
        // would drop a reachability claim the traversal actually established.
        let already_direct = set.direct.iter().any(|direct| {
            direct.subject == entry.subject
                && direct.object == entry.object
                && direct.relationship == entry.relationship
        });
        if !already_direct {
            set.transitive.push(entry);
        }
    }
    for cycle in closure.cycles {
        if !set.cycles.contains(&cycle) {
            set.cycles.push(cycle);
        }
    }
    if closure.truncated {
        set.truncated = true;
        set.truncation_reason = closure.truncation_reason;
    }
    set.validate()
}

/// Closes a set and merges the result in one step.
///
/// # Errors
///
/// Returns a dependency error when the merged set violates an invariant.
pub fn close_set(
    set: &mut DependencySet,
    source: &dyn EdgeSource,
    limits: Limits,
) -> Result<Closure> {
    let closure = close(&set.subject, source, limits);
    apply(set, closure.clone())?;
    Ok(closure)
}

/// The relationship a chain of edges is composed of, when they agree.
///
/// Used by reports to describe a path in one phrase. Returns `None` for a mixed chain,
/// because describing it as one relationship would attribute a meaning the
/// specification assigns per edge rather than per path.
#[must_use]
pub fn uniform_relationship(chain: &[Dependency]) -> Option<Relationship> {
    let mut relationships = chain.iter().map(|hop| hop.relationship);
    let first = relationships.next()?;
    relationships
        .all(|relationship| relationship == first)
        .then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::{Candidate, EvidenceRef};
    use amasario_core::{
        Basis, EntityKind, EvidenceType, LedgerSequence, Network, NetworkType, ObservationBoundary,
    };

    fn entity(id: &str) -> EntityRef {
        EntityRef::new(EntityKind::Contract, id).expect("a reference")
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

    fn edge(from: &str, to: &str, transaction: String) -> Dependency {
        let candidate = Candidate::new(
            entity(from),
            entity(to),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true));
        let classification = crate::classifier::classify(&candidate).expect("classifiable");
        Dependency {
            subject: candidate.subject,
            object: candidate.object,
            relationship: candidate.relationship,
            classes: classification.classes,
            basis: candidate.basis,
            confidence: classification.confidence,
            verification: classification.verification,
            evidence: candidate.evidence,
            path: Vec::new(),
            depth: 0,
            reason: classification.rationale,
            observed_at: candidate.boundary,
        }
    }

    fn limits(depth: usize, nodes: usize) -> Limits {
        Limits::new(depth, nodes).expect("valid bounds")
    }

    #[test]
    fn a_chain_is_closed_with_its_intermediates_in_order() {
        // A -> B -> C
        let edges = vec![
            edge("A", "B", "1".repeat(64)),
            edge("B", "C", "2".repeat(64)),
        ];
        let closure = close(&entity("A"), &edges, limits(5, 50));
        assert_eq!(
            closure.entries.len(),
            1,
            "B is one hop away and belongs to the direct partition, not here"
        );
        let reached_c = &closure.entries[0];
        assert_eq!(reached_c.object.id, "C");
        assert_eq!(reached_c.depth, 2);
        assert_eq!(
            reached_c
                .path
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["B"]
        );
        assert!(reached_c.classes.contains(&DependencyClass::Transitive));
        assert!(reached_c.classes.contains(&DependencyClass::Contract));
        assert_eq!(reached_c.confidence.level, ConfidenceLevel::Verified);
        assert_eq!(reached_c.evidence.len(), 2, "both hops are cited");
        assert!(reached_c.reason.contains("2 hop(s)"));
        assert!(!closure.truncated);
        assert!(!closure.has_cycles());
        assert_eq!(closure.deepest, 2);
    }

    #[test]
    fn the_aggregated_confidence_is_the_weakest_hop_not_the_strongest() {
        let mut weak = edge("B", "C", "2".repeat(64));
        weak.confidence = Confidence::new(
            ConfidenceLevel::MediumConfidence,
            vec!["transaction:2".to_owned()],
            Vec::new(),
        )
        .expect("a confidence");
        let edges = vec![edge("A", "B", "1".repeat(64)), weak];
        let closure = close(&entity("A"), &edges, limits(5, 50));
        assert_eq!(
            closure.entries[0].confidence.level,
            ConfidenceLevel::MediumConfidence,
            "a chain is only as strong as its weakest link"
        );
    }

    #[test]
    fn the_depth_bound_truncates_and_says_so() {
        let edges = vec![
            edge("A", "B", "1".repeat(64)),
            edge("B", "C", "2".repeat(64)),
            edge("C", "D", "3".repeat(64)),
        ];
        let closure = close(&entity("A"), &edges, limits(2, 50));
        assert!(closure.truncated);
        assert_eq!(
            closure.truncation_reason,
            Some(TruncationReason::MaxDepthReached)
        );
        assert_eq!(
            closure.entries.len(),
            1,
            "C was reached; B is direct and D is beyond the bound"
        );
        assert_eq!(closure.deepest, 2);
    }

    #[test]
    fn a_node_at_the_bound_with_no_edges_misses_nothing() {
        let edges = vec![edge("A", "B", "4".repeat(64))];
        let closure = close(&entity("A"), &edges, limits(1, 50));
        assert!(
            !closure.truncated,
            "the bound was reached but nothing beyond it existed"
        );
        assert!(
            closure.entries.is_empty(),
            "B is one hop away and is direct"
        );
        assert_eq!(closure.nodes_visited, 2);
    }

    #[test]
    fn the_node_bound_truncates_and_says_so() {
        let edges = vec![
            edge("A", "B", "5".repeat(64)),
            edge("B", "C", "6".repeat(64)),
            edge("C", "D", "7".repeat(64)),
        ];
        let closure = close(&entity("A"), &edges, limits(50, 3));
        assert_eq!(closure.limits.max_nodes, 3);
        assert!(closure.truncated);
        assert_eq!(
            closure.truncation_reason,
            Some(TruncationReason::MaxNodesReached)
        );
        assert_eq!(closure.nodes_visited, 3);
    }

    #[test]
    fn a_cycle_is_reported_with_its_edges_rather_than_collapsed() {
        // A -> B -> A
        let edges = vec![
            edge("A", "B", "8".repeat(64)),
            edge("B", "A", "9".repeat(64)),
        ];
        let closure = close(&entity("A"), &edges, limits(5, 50));
        assert!(closure.has_cycles());
        let cycle = &closure.cycles[0];
        assert_eq!(cycle.entity_ids(), vec!["A", "B", "A"]);
        assert_eq!(cycle.edges.len(), 2);
        assert!(cycle.edges[0].contains("INVOCATES"));
        assert!(
            closure.entries.is_empty(),
            "the cycle produces no dependency: B is direct and A is the subject"
        );
        assert!(!closure.truncated, "a cycle is not a truncation");
    }

    #[test]
    fn a_longer_cycle_is_reported_at_the_entity_that_closes_it() {
        let edges = vec![
            edge("A", "B", "a".repeat(64)),
            edge("B", "C", "b".repeat(64)),
            edge("C", "B", "c".repeat(64)),
        ];
        let closure = close(&entity("A"), &edges, limits(5, 50));
        assert_eq!(closure.cycles.len(), 1);
        assert_eq!(closure.cycles[0].entity_ids(), vec!["A", "B", "C", "B"]);
        assert_eq!(
            closure.entries.len(),
            1,
            "C is two hops away and is reported"
        );
    }

    #[test]
    fn the_shortest_path_wins_and_the_order_is_deterministic() {
        // A -> B -> D and A -> C -> D: D is reached in two hops by both, and the first
        // route found is kept.
        let edges = vec![
            edge("A", "B", "d".repeat(64)),
            edge("A", "C", "e".repeat(64)),
            edge("B", "D", "f".repeat(64)),
            edge("C", "D", "1a".repeat(32)),
        ];
        let first = close(&entity("A"), &edges, limits(5, 50));
        let second = close(&entity("A"), &edges, limits(5, 50));
        assert_eq!(first, second, "two runs agree");
        let reached_d = first
            .entries
            .iter()
            .find(|entry| entry.object.id == "D")
            .expect("D was reached");
        assert_eq!(reached_d.depth, 2);
        assert_eq!(
            reached_d
                .path
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["B"]
        );
    }

    #[test]
    fn an_unreached_entity_is_absent_rather_than_reported() {
        let edges = vec![
            edge("A", "B", "aa".repeat(32)),
            edge("B", "C", "bb".repeat(32)),
            edge("X", "Y", "cc".repeat(32)),
        ];
        let closure = close(&entity("A"), &edges, limits(5, 50));
        assert_eq!(closure.entries.len(), 1);
        assert_eq!(closure.entries[0].object.id, "C");
        assert_eq!(
            closure.nodes_visited, 3,
            "the subject, B and C - X and Y are not reachable from A"
        );
    }

    #[test]
    fn a_set_is_closed_and_the_edge_belongs_to_one_partition_only() {
        let mut set = crate::resolver::resolve(
            entity("A"),
            Some(boundary()),
            &[Candidate::new(
                entity("A"),
                entity("B"),
                Relationship::Invocates,
                Basis::ObservedInvocation,
                vec![
                    EvidenceRef::new(EvidenceType::Transaction, "cc".repeat(32))
                        .expect("a citation"),
                ],
            )
            .expect("a candidate")
            .observed_at(boundary())
            .with_outcome(Some(true))],
            5,
        )
        .expect("resolves");
        let edges = vec![
            edge("A", "B", "cc".repeat(32)),
            edge("B", "C", "dd".repeat(32)),
        ];
        let closure = close_set(&mut set, &edges, limits(5, 50)).expect("closes");
        assert_eq!(
            closure.entries.len(),
            1,
            "C is reached, B is already direct"
        );
        assert_eq!(set.direct.len(), 1);
        assert_eq!(set.transitive.len(), 1);
        assert_eq!(set.transitive[0].object.id, "C");
        set.validate().expect("the merged set is consistent");
    }

    #[test]
    fn a_reachability_claim_is_kept_when_only_an_intermediate_reaches_the_object() {
        // `B -> C` is in the direct partition, and `A` reaches `C` through `B`. Comparing
        // the object alone would read the second as already covered by the first and drop
        // the claim the traversal actually established.
        let mut set = crate::resolver::resolve(
            entity("A"),
            Some(boundary()),
            &[
                Candidate::new(
                    entity("A"),
                    entity("B"),
                    Relationship::Invocates,
                    Basis::ObservedInvocation,
                    vec![
                        EvidenceRef::new(EvidenceType::Transaction, "16".repeat(32))
                            .expect("a citation"),
                    ],
                )
                .expect("a candidate")
                .observed_at(boundary())
                .with_outcome(Some(true)),
                Candidate::new(
                    entity("B"),
                    entity("C"),
                    Relationship::Invocates,
                    Basis::ObservedInvocation,
                    vec![
                        EvidenceRef::new(EvidenceType::Transaction, "17".repeat(32))
                            .expect("a citation"),
                    ],
                )
                .expect("a candidate")
                .observed_at(boundary())
                .with_outcome(Some(true)),
            ],
            5,
        )
        .expect("resolves");
        assert_eq!(set.direct.len(), 2, "two edges, two subjects");

        let edges = set.direct.clone();
        close_set(&mut set, &edges, limits(5, 50)).expect("closes");
        let reached = set
            .transitive
            .iter()
            .find(|dependency| dependency.object.id == "C")
            .expect("A reaches C through B, and the claim must survive");
        assert_eq!(reached.subject.id, "A");
        assert_eq!(
            reached
                .path
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["B"]
        );
        assert_eq!(reached.depth, 2);
        set.validate().expect("the merged set is consistent");
    }

    #[test]
    fn the_node_bound_bounds_the_count_it_names() {
        // One expansion with three targets and a budget of three: enforcing the bound only
        // when a node is dequeued would let this visit four entities while reporting a
        // bound of three, which makes the bound a suggestion rather than a bound.
        let edges = vec![
            edge("A", "B", "11".repeat(32)),
            edge("A", "C", "12".repeat(32)),
            edge("A", "D", "13".repeat(32)),
        ];
        let closure = close(&entity("A"), &edges, limits(50, 3));
        assert!(closure.truncated);
        assert_eq!(
            closure.truncation_reason,
            Some(TruncationReason::MaxNodesReached)
        );
        assert_eq!(closure.nodes_visited, 3);
        assert!(
            closure.nodes_visited <= closure.limits.max_nodes,
            "a result must not report visiting more entities than the bound it names"
        );
        // Every entity that fitted is one hop away, so it belongs to the direct partition
        // and is not reported here - but the two that were visited before the bound was
        // reached were visited, and the count says so.
        assert!(
            closure.entries.is_empty(),
            "the entities that fitted are all direct: {:?}",
            closure
                .entries
                .iter()
                .map(|entry| entry.object.id.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_cycle_is_reported_even_when_the_node_bound_is_already_spent() {
        // Reporting a cycle is not visiting a node, so refusing to report one because the
        // budget ran out would hide real topology - which is exactly what the rule against
        // removing cycles exists to prevent.
        let edges = vec![
            edge("A", "B", "14".repeat(32)),
            edge("B", "A", "15".repeat(32)),
        ];
        let closure = close(&entity("A"), &edges, limits(5, 1));
        assert_eq!(closure.nodes_visited, 1, "the subject alone");
        assert!(closure.truncated);
        assert!(
            !closure.has_cycles(),
            "the loop was never entered, so there is nothing to report: {:?}",
            closure.cycles
        );

        let reached = close(&entity("A"), &edges, limits(5, 3));
        assert_eq!(reached.nodes_visited, 2);
        assert!(reached.has_cycles(), "the loop was entered and is reported");
    }

    #[test]
    fn either_bound_of_zero_is_refused() {
        Limits::new(0, 10).expect_err("a traversal that visits nothing reports silence");
        Limits::new(10, 0).expect_err("a traversal that visits nothing reports silence");
    }

    #[test]
    fn an_edge_source_can_be_a_lookup_rather_than_a_set() {
        let edges = vec![
            edge("A", "B", "ee".repeat(32)),
            edge("B", "C", "ff".repeat(32)),
        ];
        let source = |entity: &EntityRef| edges.as_slice().edges_from(entity);
        let closure = close(&entity("A"), &source, limits(5, 50));
        assert_eq!(closure.entries.len(), 1);
        assert_eq!(closure.entries[0].object.id, "C");
        assert_eq!(closure.entries[0].depth, 2);
    }

    #[test]
    fn a_uniform_chain_is_describable_by_one_relationship() {
        let edges = vec![edge("A", "B", "ff".repeat(32))];
        assert_eq!(uniform_relationship(&edges), Some(Relationship::Invocates));
        assert_eq!(uniform_relationship(&[]), None);
    }
}
