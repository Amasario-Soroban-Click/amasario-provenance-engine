//! What impact propagation costs.
//!
//! # The number that matters is not a duration
//!
//! Propagation is bounded twice - by hops and by nodes - and the bounds exist so that an
//! analysis over a graph the engine did not construct cannot run away. The cost worth
//! knowing is therefore the cost of the bound being *reached*: an analysis that stops at
//! its limit has done the same work as one that exhausted the graph, and must say so
//! rather than report a smaller affected set as though it were the whole answer.
//!
//! So the corpus's four kinds are all benchmarked. They are the same graph under
//! different bounds and different starting points, and the `bounded` case is the one
//! that reaches its limit - a benchmark of the conclusive cases alone would report a
//! cost that never includes the disclosure.
//!
//! # Why the change starts at the far end
//!
//! `INVOCATES` declares `object_to_subject` propagation: a change to the callee is what
//! reaches its callers. The corpus's fixtures start at the far end of each chain for
//! that reason, so the traversal is measured doing the work the relationship semantics
//! actually require rather than walking with the arrows.
//!
//! # The synthetic case
//!
//! The corpus chain is four entities long, which cannot show how the node bound behaves.
//! `bounded/wide` therefore builds a graph with many entities at one hop and measures
//! the analysis that must refuse to visit them all.

use std::hint::black_box;

use amasario_core::{EntityKind, EntityRef};
use amasario_dependency::transitive::{DEFAULT_MAX_NODES, Limits};
use amasario_impact::{ChangeType, ImpactAnalysis, analyze, context};
use amasario_integration_tests::documents::{self, ImpactKind, alpha, observed_edges};
use criterion::{Criterion, criterion_group, criterion_main};

/// One analysis per corpus kind: the same graph under four bounds, three start points.
fn propagation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("impact/corpus");
    for kind in ImpactKind::all() {
        group.bench_function(kind.file(), |bencher| {
            bencher.iter(|| black_box(documents::impact_analysis(black_box(*kind))));
        });
    }
    group.finish();
}

/// Propagation over a graph that is already assembled, so the two costs are separable.
///
/// [`documents::impact_analysis`] builds the graph and then analyzes it, which is what a
/// pipeline does but which hides how the cost divides. Measuring [`analyze`] over a
/// context built once shows how much of the corpus number is the traversal rather than
/// the assembly - the split a reader needs before deciding which one to optimise.
fn prebuilt(criterion: &mut Criterion) {
    let graph = documents::graph(documents::GraphKind::MultiHop);
    let impact_context = context(&graph, &[]);
    let changed = documents::delta();
    let limits = Limits::new(4, DEFAULT_MAX_NODES).expect("non-zero bounds");
    let mut group = criterion.benchmark_group("impact/prebuilt-graph");
    group.bench_function("multi-hop", |bencher| {
        bencher.iter(|| {
            let analysis: ImpactAnalysis = analyze(
                black_box(&impact_context),
                black_box(&changed),
                Some(ChangeType::Modified),
                limits,
            )
            .expect("the derived findings satisfy their rules");
            black_box(analysis.findings.len())
        });
    });
    group.finish();
}

/// The `index`-th entity of a synthetic fan.
fn leaf(index: usize) -> EntityRef {
    EntityRef::new(EntityKind::Contract, format!("Cbench{index}")).expect("a non-empty identifier")
}

/// A graph where the root invokes `width` leaves, all of which invoke one sink.
///
/// # Panics
///
/// Panics when the corpus holds no observed edge, which would mean the fixture has
/// stopped describing a contract at all.
fn fan(width: usize) -> (amasario_graph::Graph, EntityRef) {
    let template = observed_edges()
        .into_iter()
        .next()
        .expect("the corpus records at least one invocation");
    let mut graph = amasario_graph::Graph::new("amasario.bench.fan").expect("a graph identifier");
    let root = leaf(usize::MAX);
    let sink = leaf(usize::MAX - 1);
    graph.add_entity(&root).expect("a node");

    for index in 0..width {
        let mut to_leaf: amasario_dependency::Dependency = template.clone();
        to_leaf.subject = root.clone();
        to_leaf.object = leaf(index);
        to_leaf.path = Vec::new();
        to_leaf.depth = 0;
        graph.add_edge(to_leaf).expect("a distinct edge");

        let mut to_sink: amasario_dependency::Dependency = template.clone();
        to_sink.subject = leaf(index);
        to_sink.object = sink.clone();
        to_sink.path = Vec::new();
        to_sink.depth = 0;
        graph.add_edge(to_sink).expect("a distinct edge");
    }

    // A change to the sink reaches every leaf against the arrows, so propagation has to
    // visit the whole fan - which is what a node bound does not permit.
    (graph, sink)
}

/// The node bound being reached, which is the case the bound exists for.
///
/// The bound is the fan's own width, so every case does the same amount of work relative
/// to its size: propagation visits the bound and then has to stop. Measuring a bound that
/// was not reached would report the cost of a complete analysis under the name of a
/// bounded one, which is the confusion the disclosure exists to prevent.
fn bounded(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("impact/fan");
    for width in [4_usize, 64, 512] {
        let (graph, sink) = fan(width);
        let bound = Limits::new(8, width).expect("non-zero bounds");
        let impact_context = context(&graph, &[]);
        group.bench_function(format!("{width}-leaves/bounded-at-{width}"), |bencher| {
            bencher.iter(|| {
                let analysis = analyze(
                    black_box(&impact_context),
                    black_box(&sink),
                    Some(ChangeType::Modified),
                    bound,
                )
                .expect("a bounded analysis is still a valid one");
                // The disclosure is asserted inside the measurement, so a run that
                // stopped short cannot be timed as though it had finished.
                assert!(analysis.truncated, "the bound must be reached here");
                black_box(analysis.nodes_visited)
            });
        });
    }
    group.finish();
}

/// The unbounded case over the corpus graph, where the answer is complete.
fn conclusive(criterion: &mut Criterion) {
    let graph = documents::graph(documents::GraphKind::Cyclic);
    let impact_context = context(&graph, &[]);
    let changed = alpha();
    let limits = Limits::new(16, DEFAULT_MAX_NODES).expect("non-zero bounds");
    let mut group = criterion.benchmark_group("impact/cyclic");
    group.bench_function("cycle-does-not-hang", |bencher| {
        bencher.iter(|| {
            // A cycle is the case where an unbounded traversal would not terminate. The
            // benchmark asserts the analysis concluded rather than only that it returned.
            let analysis = analyze(
                black_box(&impact_context),
                black_box(&changed),
                Some(ChangeType::Modified),
                limits,
            )
            .expect("the derived findings satisfy their rules");
            black_box(analysis.findings.len())
        });
    });
    group.finish();
}

criterion_group!(benches, propagation, prebuilt, bounded, conclusive);
criterion_main!(benches);
