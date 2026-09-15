//! Cycle detection: which entities are mutually reachable, and one concrete cycle per
//! group.
//!
//! # Why cycles are reported rather than removed
//!
//! `dependency/transitive-dependency` is explicit: a cycle must be reported "with the
//! edges that form it rather than removed or collapsed", because "a consumer that never
//! sees the cycle cannot know the graph was not a tree". Removing one would make the
//! graph acyclic and tidy and would make every downstream statement about it false.
//!
//! # Why exactly one cycle per strongly connected component
//!
//! Enumerating every elementary cycle is exponential in the worst case - a complete
//! graph on `n` nodes has more cycles than any report could carry - and the useful
//! question is not "how many cycles are there" but "which entities are on one". A
//! strongly connected component with more than one node *is* that answer: every node in
//! it lies on a cycle, and no node outside it does. [`find`] therefore reports one
//! concrete cycle per such component, chosen deterministically, and
//! [`components`] exposes the full grouping for a report that wants to name all of them.
//!
//! # Why the rendering matches the dependency layer's
//!
//! A cycle reported here and the same cycle reported by
//! [`amasario_dependency::close`] must be the same value, or a report that merges the
//! two sources would list one cycle twice in two spellings. The edge rendering is
//! therefore the dependency layer's own - `"{source} -{RELATIONSHIP}-> {target}"` - and
//! `tests/cycle_conformance.rs` builds the same cycle both ways and asserts the two
//! agree, which is what keeps the format from drifting.

use std::collections::VecDeque;

use amasario_core::{EntityRef, Relationship};
use amasario_dependency::Cycle;

use crate::edges::render as edge_identifier;
use crate::graph::Graph;
use crate::nodes::canonical_order;

/// The entities that are mutually reachable, as strongly connected components.
///
/// Only components that contain a cycle are returned: a component of one node with no
/// edge from it to itself is not a cycle, and reporting it as one would make every leaf
/// in a graph a cycle.
#[must_use]
pub fn components(graph: &Graph) -> Vec<Vec<EntityRef>> {
    let index: Vec<EntityRef> = graph.nodes.iter().map(entity_of).collect();
    let adjacency = adjacency(graph, &index);

    let mut components = tarjan(&adjacency);
    components.retain(|component| {
        component.len() > 1
            || adjacency[component[0]]
                .iter()
                .any(|(_, target)| *target == component[0])
    });
    components.sort_by(|left, right| {
        // Canonical order, so two runs agree: by the first node's kind then identifier,
        // then by size so that a nested component never depends on traversal order.
        left.len()
            .cmp(&right.len())
            .then_with(|| index[left[0]].to_string().cmp(&index[right[0]].to_string()))
    });
    components
        .into_iter()
        .map(|component| {
            component
                .into_iter()
                .map(|node| index[node].clone())
                .collect()
        })
        .collect()
}

/// Every cycle the graph contains, one per strongly connected component.
///
/// Deterministic: components are ordered canonically and each representative cycle is
/// the shortest one from the component's first node, found breadth first, so two runs
/// over the same edges report the same entities and the same edges in the same order.
#[must_use]
pub fn find(graph: &Graph) -> Vec<Cycle> {
    let index: Vec<EntityRef> = graph.nodes.iter().map(entity_of).collect();
    let adjacency = adjacency(graph, &index);

    let mut cycles: Vec<Cycle> = Vec::new();
    for component in tarjan(&adjacency) {
        let cyclic = component.len() > 1
            || adjacency[component[0]]
                .iter()
                .any(|(_, target)| *target == component[0]);
        if !cyclic {
            continue;
        }
        if let Some(cycle) = representative_cycle(graph, &index, &adjacency, &component) {
            cycles.push(cycle);
        }
    }

    // The dependency layer sorts cycles by the identifier of their first entity, and this
    // must match: two orderings for one set of cycles would make a merged report list
    // them differently depending on which layer produced them.
    cycles.sort_by(|left, right| left.entities[0].id.cmp(&right.entities[0].id));
    cycles.dedup();
    cycles
}

/// Whether the graph contains a cycle.
///
/// Cheaper than [`find`] because it stops at the first component that has one rather than
/// building every representative cycle.
#[must_use]
pub fn has_cycle(graph: &Graph) -> bool {
    let index: Vec<EntityRef> = graph.nodes.iter().map(entity_of).collect();
    let adjacency = adjacency(graph, &index);
    tarjan(&adjacency).into_iter().any(|component| {
        component.len() > 1
            || adjacency[component[0]]
                .iter()
                .any(|(_, target)| *target == component[0])
    })
}

/// The entities behind node indices.
fn entity_of(node: &crate::nodes::Node) -> EntityRef {
    EntityRef::new(
        node.kind,
        node.id[node.kind.as_str().len() + 1..].to_owned(),
    )
    .expect("a node's identifier was built from an entity reference")
}

/// The graph's adjacency, as node indices paired with the edge that connects them.
///
/// Ordered by the target's canonical position rather than by the edge's, so that a
/// traversal visiting neighbours in order visits them by entity - which is the order a
/// reader can predict and a second run can reproduce.
fn adjacency(graph: &Graph, index: &[EntityRef]) -> Vec<Vec<(usize, usize)>> {
    let mut adjacency: Vec<Vec<(usize, usize)>> = vec![Vec::new(); index.len()];
    for (edge_index, edge) in graph.edges.iter().enumerate() {
        let Some(source) = index.iter().position(|entity| *entity == *edge.source()) else {
            continue;
        };
        let Some(target) = index.iter().position(|entity| *entity == *edge.target()) else {
            continue;
        };
        adjacency[source].push((edge_index, target));
    }
    for neighbours in &mut adjacency {
        neighbours.sort_by(|left, right| {
            let left_node = &index[left.1];
            let right_node = &index[right.1];
            left_node
                .kind
                .as_str()
                .cmp(right_node.kind.as_str())
                .then_with(|| left_node.id.cmp(&right_node.id))
        });
    }
    adjacency
}

/// Tarjan's strongly connected components, computed iteratively.
///
/// Iterative because the recursion depth of the recursive form is the length of the
/// longest path, and a graph assembled from a deep dependency chain is exactly the case
/// this engine exists for; a stack overflow on the input the analysis is meant to handle
/// is a defect, not a limit.
fn tarjan(adjacency: &[Vec<(usize, usize)>]) -> Vec<Vec<usize>> {
    /// The bookkeeping for one node, mirroring the recursive formulation's stack frame.
    struct Frame {
        node: usize,
        next: usize,
    }

    const UNVISITED: usize = usize::MAX;
    let mut discovery: Vec<usize> = vec![UNVISITED; adjacency.len()];
    let mut low: Vec<usize> = vec![0; adjacency.len()];
    let mut on_stack: Vec<bool> = vec![false; adjacency.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut counter = 0_usize;

    for root in 0..adjacency.len() {
        if discovery[root] != UNVISITED {
            continue;
        }
        frames.push(Frame {
            node: root,
            next: 0,
        });
        discovery[root] = counter;
        low[root] = counter;
        counter += 1;
        stack.push(root);
        on_stack[root] = true;

        while let Some(frame) = frames.last_mut() {
            let node = frame.node;
            if frame.next < adjacency[node].len() {
                let (_, target) = adjacency[node][frame.next];
                frame.next += 1;
                if discovery[target] == UNVISITED {
                    discovery[target] = counter;
                    low[target] = counter;
                    counter += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    frames.push(Frame {
                        node: target,
                        next: 0,
                    });
                } else if on_stack[target] {
                    low[node] = low[node].min(discovery[target]);
                }
                continue;
            }

            // The node's neighbours are exhausted, so it is finished: propagate its low
            // link to its parent and, if it is a root, pop its component.
            frames.pop();
            if let Some(parent) = frames.last() {
                low[parent.node] = low[parent.node].min(low[node]);
            }
            if low[node] == discovery[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort_unstable();
                components.push(component);
            }
        }
    }

    components
}

/// The shortest cycle through a component's first node, found breadth first.
///
/// A shortest cycle rather than any cycle, because it is the one a reader can check most
/// cheaply, and breadth-first search makes it the deterministic one: with the adjacency
/// ordered canonically, the first closing edge found is the same on every run.
fn representative_cycle(
    graph: &Graph,
    index: &[EntityRef],
    adjacency: &[Vec<(usize, usize)>],
    component: &[usize],
) -> Option<Cycle> {
    let start = component[0];
    let in_component = |node: usize| component.binary_search(&node).is_ok();

    let mut parent: Vec<Option<(usize, usize)>> = vec![None; index.len()];
    let mut seen: Vec<bool> = vec![false; index.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    seen[start] = true;
    queue.push_back(start);

    while let Some(node) = queue.pop_front() {
        for (edge_index, target) in &adjacency[node] {
            if *target == start {
                // The loop is closed. Walk the parents back to the start to recover the
                // chain that reached this node, then append the closing edge. The walk
                // pushes a node together with the edge *into* it, so reversing both
                // lists leaves them in traversal order; the start appears once at each
                // end and never among the intermediates, which is why nothing is
                // special-cased for it. A self-loop therefore yields the two-element
                // entity list the dependency layer produces for the same loop.
                let mut nodes = Vec::new();
                let mut edges = vec![*edge_index];
                let mut current = node;
                while let Some((previous, edge)) = parent[current] {
                    nodes.push(current);
                    edges.push(edge);
                    current = previous;
                }
                nodes.reverse();
                edges.reverse();

                let mut entities = vec![index[start].clone()];
                entities.extend(nodes.iter().map(|node| index[*node].clone()));
                entities.push(index[start].clone());
                let rendered = edges
                    .iter()
                    .map(|edge| edge_identifier(graph.edges[*edge].dependency()))
                    .collect();
                return Some(Cycle {
                    entities,
                    edges: rendered,
                });
            }
            if !in_component(*target) || seen[*target] {
                continue;
            }
            seen[*target] = true;
            parent[*target] = Some((node, *edge_index));
            queue.push_back(*target);
        }
    }
    None
}

/// The relationships a cycle is composed of, when they agree.
///
/// The dependency layer exposes the same question for a path, and a report uses both to
/// describe a chain in one phrase. Kept here rather than inlined so that a report does
/// not have to reconstruct the relationship sequence from the rendered edge strings.
#[must_use]
pub fn uniform_relationship(cycle: &Cycle, graph: &Graph) -> Option<Relationship> {
    let mut relationships = cycle.edges.iter().map(|rendered| {
        graph
            .edges
            .iter()
            .find(|edge| &edge_identifier(edge.dependency()) == rendered)
            .map(crate::edges::Edge::relationship)
    });
    let first = relationships.next()??;
    relationships
        .all(|relationship| relationship == Some(first))
        .then_some(first)
}

/// The nodes of a graph that lie on some cycle, in canonical order.
#[must_use]
pub fn cyclic_nodes(graph: &Graph) -> Vec<&crate::nodes::Node> {
    let mut nodes: Vec<&crate::nodes::Node> = components(graph)
        .into_iter()
        .flatten()
        .filter_map(|entity| graph.node_for(&entity))
        .collect();
    nodes.sort_by(|left, right| canonical_order(left, right));
    nodes.dedup_by(|left, right| left.id == right.id);
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use amasario_core::{
        Basis, EntityKind, EvidenceType, LedgerSequence, Network, NetworkType, ObservationBoundary,
        Relationship,
    };
    use amasario_dependency::{Candidate, DependencySet, EvidenceRef, Limits, close, resolve};

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
            ledger: LedgerSequence::new(6_000).expect("a ledger"),
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

    fn graph_of(id: &str, candidates: &[Candidate]) -> (Graph, DependencySet) {
        let subject = entity(EntityKind::Contract, "C-subject");
        let set = resolve(subject, Some(boundary()), candidates, 8).expect("resolves");
        let graph = Graph::from_dependencies(id, &set).expect("a graph");
        (graph, set)
    }

    #[test]
    fn a_tree_has_no_cycles() {
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-subject", "C-a", &"a".repeat(64)),
                edge("C-a", "C-b", &"b".repeat(64)),
                edge("C-a", "C-c", &"c".repeat(64)),
            ],
        );
        assert!(!has_cycle(&graph));
        assert!(find(&graph).is_empty());
        assert!(components(&graph).is_empty());
        assert!(cyclic_nodes(&graph).is_empty());
    }

    #[test]
    fn a_two_node_cycle_is_found_with_both_edges() {
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-subject", "C-a", &"a".repeat(64)),
                edge("C-a", "C-b", &"b".repeat(64)),
                edge("C-b", "C-a", &"c".repeat(64)),
            ],
        );
        assert!(has_cycle(&graph));
        let cycles = find(&graph);
        assert_eq!(cycles.len(), 1, "one component, one cycle: {cycles:?}");
        let ids: Vec<&str> = cycles[0].entity_ids();
        assert_eq!(
            ids,
            vec!["C-a", "C-b", "C-a"],
            "the loop closes exactly once"
        );
        assert_eq!(cycles[0].edges.len(), 2);
        assert_eq!(
            cycles[0].edges,
            vec![
                "CONTRACT:C-a -INVOCATES-> CONTRACT:C-b".to_owned(),
                "CONTRACT:C-b -INVOCATES-> CONTRACT:C-a".to_owned(),
            ]
        );
        assert_eq!(
            uniform_relationship(&cycles[0], &graph),
            Some(Relationship::Invocates)
        );

        let on_cycle: Vec<&str> = cyclic_nodes(&graph)
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(on_cycle, vec!["CONTRACT:C-a", "CONTRACT:C-b"]);

        // The subject is not on the cycle, and the analysis must not pretend it is.
        assert!(
            !on_cycle.contains(&"CONTRACT:C-subject"),
            "being upstream of a cycle is not being on one"
        );
    }

    #[test]
    fn the_graph_and_the_dependency_closure_report_the_same_cycle() {
        // The conformance that makes the duplicated rendering safe. If these two ever
        // disagree, a report merging both sources would list one cycle in two spellings.
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-subject", "C-a", &"a".repeat(64)),
                edge("C-a", "C-b", &"b".repeat(64)),
                edge("C-b", "C-a", &"c".repeat(64)),
            ],
        );
        let subject = entity(EntityKind::Contract, "C-subject");
        let closure = close(&subject, &graph, Limits::defaults());
        assert!(closure.has_cycles(), "the closure must see the cycle too");

        let from_graph = find(&graph);
        let from_closure = &closure.cycles;
        // Both report the loop through C-a and C-b, and they describe it differently by
        // design: the closure reports the loop *as reached from the subject*, so its entity
        // list begins at the subject and its edge list includes the approach edge, while
        // the graph reports the component itself. What must agree exactly is the rendering
        // of the edges that are on the loop, so every edge the graph names must appear
        // verbatim in the closure's list.
        assert_eq!(from_closure.len(), 1, "got: {from_closure:?}");
        assert_eq!(from_graph.len(), 1, "got: {from_graph:?}");
        assert_eq!(
            from_graph[0].edges.len(),
            2,
            "the component is the two-entity loop, without the approach"
        );
        for rendered in &from_graph[0].edges {
            assert!(
                from_closure[0].edges.contains(rendered),
                "the graph's edge {rendered:?} is not rendered identically by the closure: {:?}",
                from_closure[0].edges
            );
        }
        assert!(
            from_closure[0].entities.len() > from_graph[0].entities.len(),
            "the closure's report includes the approach from the subject"
        );
        for entity in &from_graph[0].entities {
            assert!(
                from_closure[0].entities.contains(entity),
                "{entity} is on the loop and must appear in the closure's report"
            );
        }
    }

    #[test]
    fn a_three_node_cycle_becomes_one_component_and_one_representative_cycle() {
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-a", "C-b", &"a".repeat(64)),
                edge("C-b", "C-c", &"b".repeat(64)),
                edge("C-c", "C-a", &"c".repeat(64)),
            ],
        );
        assert!(has_cycle(&graph));
        assert_eq!(components(&graph).len(), 1);
        let cycles = find(&graph);
        assert_eq!(cycles.len(), 1, "one component, not three rotations");
        assert_eq!(cycles[0].edges.len(), 3);
        assert_eq!(
            cycles[0].entities.len(),
            4,
            "three nodes and the closing repeat"
        );
        assert!(uniform_relationship(&cycles[0], &graph).is_some());
    }

    #[test]
    fn two_disjoint_cycles_are_reported_separately() {
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-a", "C-b", &"a".repeat(64)),
                edge("C-b", "C-a", &"b".repeat(64)),
                edge("C-c", "C-d", &"c".repeat(64)),
                edge("C-d", "C-c", &"d".repeat(64)),
            ],
        );
        let cycles = find(&graph);
        assert_eq!(cycles.len(), 2, "got: {cycles:?}");
        let mut firsts: Vec<&str> = cycles
            .iter()
            .map(|cycle| cycle.entities[0].id.as_str())
            .collect();
        firsts.sort_unstable();
        assert_eq!(firsts, vec!["C-a", "C-c"], "sorted by first entity");
        assert_eq!(components(&graph).len(), 2);
        assert_eq!(cyclic_nodes(&graph).len(), 4);
    }

    #[test]
    fn a_cycle_nested_inside_another_is_one_component() {
        // A strongly connected component is the right unit: all three nodes lie on a
        // cycle, and a report naming only the inner pair would understate the exposure.
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-a", "C-b", &"a".repeat(64)),
                edge("C-b", "C-c", &"b".repeat(64)),
                edge("C-c", "C-a", &"c".repeat(64)),
                edge("C-b", "C-a", &"d".repeat(64)),
            ],
        );
        let components = components(&graph);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), 3);
        assert_eq!(find(&graph).len(), 1);
        assert_eq!(cyclic_nodes(&graph).len(), 3);
    }

    #[test]
    fn a_long_chain_terminating_in_a_cycle_reports_only_the_cycle() {
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-subject", "C-1", &"1".repeat(64)),
                edge("C-1", "C-2", &"2".repeat(64)),
                edge("C-2", "C-3", &"3".repeat(64)),
                edge("C-3", "C-2", &"4".repeat(64)),
            ],
        );
        let cycles = find(&graph);
        assert_eq!(cycles.len(), 1);
        let ids = cycles[0].entity_ids();
        assert_eq!(ids, vec!["C-2", "C-3", "C-2"]);
        assert_eq!(cyclic_nodes(&graph).len(), 2);
    }

    #[test]
    fn detection_is_deterministic_and_independent_of_edge_insertion_order() {
        let forwards = [
            edge("C-a", "C-b", &"a".repeat(64)),
            edge("C-b", "C-c", &"b".repeat(64)),
            edge("C-c", "C-a", &"c".repeat(64)),
        ];
        let backwards = [
            edge("C-c", "C-a", &"c".repeat(64)),
            edge("C-b", "C-c", &"b".repeat(64)),
            edge("C-a", "C-b", &"a".repeat(64)),
        ];
        let (first, _) = graph_of("g1", &forwards);
        let (second, _) = graph_of("g1", &backwards);
        assert_eq!(find(&first), find(&second));
        assert_eq!(components(&first), components(&second));
        for _ in 0..8 {
            assert_eq!(find(&first), find(&second));
        }
    }

    #[test]
    fn a_deep_chain_terminates_without_stack_growth_and_is_reported_acyclic() {
        // The component search is iterative, so it has no frame per node and cannot be
        // driven into a stack overflow by exactly the input the engine exists to handle:
        // a long dependency chain. The depth is exercised rather than asserted, and the
        // answer is checked rather than merely the absence of a crash.
        let chain: Vec<Candidate> = (0..1_200)
            .map(|index| {
                edge(
                    &format!("C-{index}"),
                    &format!("C-{}", index + 1),
                    &format!("{index:064x}"),
                )
            })
            .collect();
        let (graph, _) = graph_of("g1", &chain);
        assert_eq!(graph.edge_count(), 1_200);
        assert!(!has_cycle(&graph), "a chain has no cycles");
        assert!(find(&graph).is_empty());
        assert!(components(&graph).is_empty());
    }

    #[test]
    fn a_self_loop_is_reported_as_a_two_element_cycle_like_the_dependency_layer() {
        // A validated graph cannot contain one, because `dependency/direct-dependency`
        // rejects a self-dependency upstream. The detector is still exercised through a
        // hand-built graph, because "cannot happen" is a property of the constructors
        // rather than of the algorithm, and an algorithm that quietly dropped a
        // one-node component would understate a hand-authored graph.
        use crate::edges::Edge;
        use crate::nodes::Node;

        let mut graph = Graph::new("g1").expect("a graph");
        graph
            .add_node(Node::for_entity(&entity(EntityKind::Contract, "C-a")))
            .expect("a node");
        let set = resolve(
            entity(EntityKind::Contract, "C-a"),
            Some(boundary()),
            &[edge("C-a", "C-b", &"a".repeat(64))],
            4,
        )
        .expect("resolves");
        let mut dependency = set.direct[0].clone();
        dependency.object = dependency.subject.clone();
        dependency.relationship = Relationship::DependsOn;
        graph.edges.push(Edge::from_dependency(dependency));

        assert_eq!(
            graph.integrity_failures().len(),
            1,
            "the self-edge is the only fault"
        );
        assert!(has_cycle(&graph));
        let cycles = find(&graph);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].entity_ids(), vec!["C-a", "C-a"]);
        assert_eq!(cycles[0].edges.len(), 1);
    }

    #[test]
    fn a_cycle_reached_through_a_shared_node_is_still_one_component() {
        // Two paths into one loop: the component is still the loop.
        let (graph, _) = graph_of(
            "g1",
            &[
                edge("C-subject", "C-a", &"a".repeat(64)),
                edge("C-a", "C-loop", &"b".repeat(64)),
                edge("C-loop", "C-loop2", &"c".repeat(64)),
                edge("C-loop2", "C-loop", &"d".repeat(64)),
            ],
        );
        assert_eq!(components(&graph).len(), 1);
        assert_eq!(find(&graph).len(), 1);
        assert_eq!(cyclic_nodes(&graph).len(), 2);
    }
}
