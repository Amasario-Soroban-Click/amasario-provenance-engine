//! What assembling and traversing the dependency graph costs.
//!
//! # Why assembly is benchmarked together with traversal
//!
//! A graph is not a value the engine reads; it is one it builds, and the build is where
//! the two rules that make traversal meaningful are applied: an edge that dangles is
//! refused rather than repaired by inventing a node, and an edge identifier is derived
//! from its endpoints and its relationship so that a diff can tell an unchanged edge
//! from a replaced one. A benchmark of traversal alone would report a cost the pipeline
//! never pays on its own, and would hide the cost of the checks that make the traversal
//! trustworthy.
//!
//! # The four traversals, which answer four different questions
//!
//! * `walk` - what this entity reaches, which is the dependency question.
//! * `walk_reverse` - what reaches this entity, which is the impact question, and the
//!   one that must not be the forward walk with the edges reversed by accident.
//! * `all_paths_bounded` - by what routes, which is what an impact finding cites.
//! * `find_cycles` - whether the answer is a tree at all, which is the case a report
//!   must disclose rather than collapse.
//!
//! `walk_reverse` is included even though it shares an implementation, because the two
//! directions visit different subgraphs on any graph that is not a chain, and a change
//! that made the reverse view symmetric would be invisible in a single number.

use std::hint::black_box;

use amasario_core::{EntityKind, EntityRef};
use amasario_dependency::Dependency;
use amasario_dependency::transitive::{DEFAULT_MAX_NODES, Limits, MAX_PERMITTED_DEPTH};
use amasario_graph::{Graph, all_paths_bounded, find_cycles, has_cycle, walk, walk_reverse};
use amasario_integration_tests::documents::{self, GraphKind, alpha, observed_edges};
use criterion::{Criterion, criterion_group, criterion_main};

/// Assembling one of the corpus graphs from the dependency entries that describe it.
fn assembly(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("graph/assemble");
    for kind in GraphKind::all() {
        group.bench_function(kind.file(), |bencher| {
            bencher.iter(|| black_box(documents::graph(black_box(*kind))));
        });
    }
    group.finish();
}

/// The document projection, which is what a consumer reads rather than the graph itself.
fn projection(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("graph/project-document");
    for kind in GraphKind::all() {
        group.bench_function(kind.file(), |bencher| {
            bencher.iter(|| black_box(documents::graph_document(black_box(*kind))));
        });
    }
    group.finish();
}

/// The forward walk, the reverse walk, and the bounded path search over one graph.
fn traversal(criterion: &mut Criterion) {
    let graph = documents::graph(GraphKind::MultiHop);
    let subject = alpha();
    let last = documents::delta();
    let limits = Limits::new(8, DEFAULT_MAX_NODES).expect("non-zero bounds");

    let mut group = criterion.benchmark_group("graph/traverse/4-nodes");
    group.bench_function("walk-forward", |bencher| {
        bencher.iter(|| black_box(walk(black_box(&graph), black_box(&subject), limits)));
    });
    group.bench_function("walk-reverse", |bencher| {
        bencher.iter(|| black_box(walk_reverse(black_box(&graph), black_box(&last), limits)));
    });
    group.bench_function("all-paths", |bencher| {
        bencher.iter(|| {
            black_box(all_paths_bounded(
                black_box(&graph),
                black_box(&subject),
                black_box(&last),
                limits,
                amasario_graph::DEFAULT_MAX_PATHS,
            ))
        });
    });
    group.bench_function("find-cycles", |bencher| {
        bencher.iter(|| black_box(find_cycles(black_box(&graph))));
    });
    group.finish();
}

/// The cycle question on the graph that has one, which is the case worth timing.
///
/// `cyclic` holds a two-edge cycle, so the detector has to do work rather than prove an
/// absence; a benchmark over an acyclic graph would measure the cheap answer.
fn cycles(criterion: &mut Criterion) {
    let cyclic = documents::graph(GraphKind::Cyclic);
    let acyclic = documents::graph(GraphKind::MultiHop);
    let mut group = criterion.benchmark_group("graph/cycles");
    group.bench_function("cyclic", |bencher| {
        bencher.iter(|| black_box(has_cycle(black_box(&cyclic))));
    });
    group.bench_function("acyclic", |bencher| {
        bencher.iter(|| black_box(has_cycle(black_box(&acyclic))));
    });
    group.finish();
}

/// The `index`-th entity of a synthetic chain.
fn link(index: usize) -> EntityRef {
    EntityRef::new(EntityKind::Contract, format!("Cbench{index}")).expect("a non-empty identifier")
}

/// A linear chain of `length` edges, shaped like the corpus's own `INVOCATES` edge.
///
/// # Panics
///
/// Panics when the corpus holds no observed edge, which would mean the fixture has
/// stopped describing a contract at all.
fn chain_graph(length: usize) -> Graph {
    let template = observed_edges()
        .into_iter()
        .next()
        .expect("the corpus records at least one invocation");
    let mut graph = Graph::new("amasario.bench.chain").expect("a graph identifier");
    let mut previous = link(0);
    graph.add_entity(&previous).expect("a node");

    for index in 0..length {
        let mut edge: Dependency = template.clone();
        edge.subject = previous.clone();
        edge.object = link(index + 1);
        edge.path = Vec::new();
        edge.depth = 0;
        graph.add_edge(edge).expect("a distinct edge");
        previous = link(index + 1);
    }
    graph
}

/// Walk cost as a function of how far there is to walk.
///
/// The corpus's graph has four entities, which cannot show scaling. The chain is built
/// from the corpus's own edge shape, so the same code path is measured at sizes the
/// fixture deliberately does not reach. The longest case is
/// [`MAX_PERMITTED_DEPTH`] hops, because a larger bound is clamped rather than honoured:
/// a longer chain would report the same time and answer nothing.
fn chain_traversal(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("graph/traverse/chain");
    for length in [4_usize, 8, 16, MAX_PERMITTED_DEPTH] {
        let graph = chain_graph(length);
        let start = link(0);
        let end = link(length);
        let limits = Limits::new(length, DEFAULT_MAX_NODES).expect("non-zero bounds");
        group.bench_function(format!("{length}-edge/forward"), |bencher| {
            bencher.iter(|| {
                let walked = walk(black_box(&graph), black_box(&start), limits);
                assert!(
                    !walked.is_inconclusive(),
                    "the bound reaches the end of the chain"
                );
                black_box(walked.entries.len())
            });
        });
        group.bench_function(format!("{length}-edge/reverse"), |bencher| {
            bencher.iter(|| {
                let walked = walk_reverse(black_box(&graph), black_box(&end), limits);
                assert!(
                    !walked.is_inconclusive(),
                    "the bound reaches the start of the chain"
                );
                black_box(walked.entries.len())
            });
        });
    }
    group.finish();
}

/// A breadth-first tree, so that the node bound rather than the hop bound is what runs out.
///
/// `depth` is bounded by [`MAX_PERMITTED_DEPTH`] and the shape is branching, which is the
/// case a chain cannot produce: the number of entities reachable at a *shallow* depth is
/// what makes the node bound necessary, and a chain of the same depth visits one entity
/// per level.
fn tree_graph(depth: usize, branching: usize) -> Graph {
    let template = observed_edges()
        .into_iter()
        .next()
        .expect("the corpus records at least one invocation");
    let mut graph = Graph::new("amasario.bench.tree").expect("a graph identifier");
    let mut next = 0_usize;
    let mut frontier = vec![link(next)];
    graph.add_entity(&frontier[0]).expect("a node");

    for _ in 0..depth {
        let mut children = Vec::with_capacity(frontier.len() * branching);
        for parent in &frontier {
            for _ in 0..branching {
                next += 1;
                let child = link(next);
                let mut edge: Dependency = template.clone();
                edge.subject = parent.clone();
                edge.object = child.clone();
                edge.path = Vec::new();
                edge.depth = 0;
                graph.add_edge(edge).expect("a distinct edge");
                children.push(child);
            }
        }
        frontier = children;
    }
    graph
}

/// Walk cost as a function of how many entities there are, at one depth.
///
/// The hop bound is the tree's own depth, so every case is a complete walk and the
/// increase is the node count rather than the distance.
fn tree_traversal(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("graph/traverse/tree");
    for depth in [3_usize, 4, 5] {
        let graph = tree_graph(depth, 4);
        let start = link(0);
        let limits = Limits::new(depth, DEFAULT_MAX_NODES).expect("non-zero bounds");
        let nodes = graph.node_count();
        group.bench_function(format!("{nodes}-nodes/depth-{depth}"), |bencher| {
            bencher.iter(|| black_box(walk(black_box(&graph), black_box(&start), limits)));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    assembly,
    projection,
    traversal,
    cycles,
    chain_traversal,
    tree_traversal
);
criterion_main!(benches);
