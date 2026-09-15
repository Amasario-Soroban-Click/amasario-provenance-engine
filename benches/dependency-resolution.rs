//! What dependency resolution costs.
//!
//! # The two costs, which are different problems
//!
//! Resolution is detection plus classification plus validation: turn the recorded
//! invocations into candidates, decide whether a rule permits each claim, and check the
//! resulting set against the partition rule. That is linear in the observations and is
//! what an analysis of one contract pays.
//!
//! Closure is the other cost. Closing a set under a bounded breadth-first traversal is
//! the step that turns what a subject reaches into what it reaches *through*
//! intermediates, and its cost grows with the reachable subgraph rather than with the
//! subject's own observations. The two are benchmarked separately because a change that
//! makes one faster can make the other slower, and a single number would not show it.
//!
//! # Why the depth bound appears in the benchmark names
//!
//! The bound is not a tuning parameter here: it is part of the result. A traversal that
//! stopped early must say so with a reason, and the cost of a bounded run is the cost of
//! a run whose answer is qualified. Benchmarking one depth would imply the others are
//! the same function of it, which is the assumption a bounded search exists to refuse.
//!
//! # Why one case is synthetic
//!
//! The corpus's sets are small by design - each one demonstrates one rule - and its
//! graph has four entities, which cannot show how traversal scales. The `chain` group
//! therefore builds a linear chain of a chosen length out of the corpus's own edge
//! shape, so the same code path is measured at a size the fixture deliberately does not
//! reach. The dependency is real, not a stub: `close` receives the same
//! `EdgeSource` implementation the pipeline hands it.

use std::hint::black_box;

use amasario_core::{EntityKind, EntityRef};
use amasario_dependency::Dependency;
use amasario_dependency::transitive::{
    DEFAULT_MAX_NODES, Limits, MAX_PERMITTED_DEPTH, close, close_set,
};
use amasario_integration_tests::documents::{
    self, CALL_BRAVO, SetKind, alpha, dependency_set, observed_edges,
};
use criterion::{Criterion, criterion_group, criterion_main};

/// Detection, classification, resolution and validation over each recorded set.
fn resolve(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("resolution/set");
    for kind in SetKind::all() {
        let edges = documents::set(*kind).len();
        group.bench_function(format!("{}/{}edge", kind.file(), edges), |bencher| {
            bencher.iter(|| black_box(documents::set(black_box(*kind))));
        });
    }
    group.finish();
}

/// The closure itself, at four bounds, over the corpus's observed edge union.
///
/// The source is the union rather than the set, because that is what a real analysis
/// walks: a contract's dependency set holds only its own observations, and a second hop
/// is by definition somebody else's.
fn close_at_depth(criterion: &mut Criterion) {
    let edges = observed_edges();
    let subject = alpha();
    let mut group = criterion.benchmark_group("resolution/closure");
    for depth in [1_usize, 2, 3, 8] {
        let limits = Limits::new(depth, DEFAULT_MAX_NODES).expect("non-zero bounds");
        group.bench_function(format!("depth-{depth}"), |bencher| {
            bencher.iter(|| black_box(close(black_box(&subject), black_box(&edges), limits)));
        });
    }
    group.finish();
}

/// Merging a closure back into a set, which is where the partition rule is enforced.
///
/// Separate from computing the closure because it is not free: every entry is compared
/// against the direct partition to keep one relationship out of two partitions, and the
/// merged set is validated. A benchmark of the closure alone would report a cost the
/// engine never pays on its own.
fn merge(criterion: &mut Criterion) {
    let edges = observed_edges();
    let limits = Limits::new(3, DEFAULT_MAX_NODES).expect("non-zero bounds");
    let mut group = criterion.benchmark_group("resolution/merge");
    group.bench_function("direct-plus-closure", |bencher| {
        bencher.iter(|| {
            let mut set = dependency_set(alpha(), &[CALL_BRAVO], 1);
            close_set(black_box(&mut set), black_box(&edges), limits)
                .expect("the merged set satisfies its rules");
            black_box(set.len())
        });
    });
    group.finish();
}

/// A linear chain of `length` edges, shaped like the corpus's own `INVOCATES` edge.
///
/// # Panics
///
/// Panics when the corpus holds no observed edge, which would mean the fixture has
/// stopped describing a contract at all.
fn chain(length: usize) -> (EntityRef, Vec<Dependency>) {
    let template = observed_edges()
        .into_iter()
        .next()
        .expect("the corpus records at least one invocation");
    let mut edges: Vec<Dependency> = Vec::with_capacity(length);

    for index in 0..length {
        let mut edge = template.clone();
        edge.subject = link(index);
        edge.object = link(index + 1);
        // The path and depth describe a *transitive* entry; a direct hop has neither.
        edge.path = Vec::new();
        edge.depth = 0;
        edges.push(edge);
    }

    (link(0), edges)
}

/// The `index`-th entity of a synthetic chain.
fn link(index: usize) -> EntityRef {
    EntityRef::new(EntityKind::Contract, format!("Cbench{index}")).expect("a non-empty identifier")
}

/// Traversal cost as a function of how far there is to walk.
///
/// The longest case is [`MAX_PERMITTED_DEPTH`] hops, because that is the engine's own
/// ceiling: a larger bound is clamped rather than honoured, so a longer chain would
/// measure the clamp and report the same time as the shorter one. Measuring up to the
/// ceiling and saying so is more use than a larger number that answers nothing.
fn chain_traversal(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("resolution/chain");
    for length in [4_usize, 8, 16, MAX_PERMITTED_DEPTH] {
        let (subject, edges) = chain(length);
        let limits = Limits::new(length, DEFAULT_MAX_NODES).expect("non-zero bounds");
        group.bench_function(format!("{length}-hop"), |bencher| {
            bencher.iter(|| {
                let closure = close(black_box(&subject), black_box(&edges), limits);
                // A run that stopped early would report a shorter chain's cost under a
                // longer chain's name, so the depth reached is asserted inside the
                // measurement rather than only its duration.
                assert!(!closure.truncated, "the bound reaches the end of the chain");
                black_box(closure.deepest)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, resolve, close_at_depth, merge, chain_traversal);
criterion_main!(benches);
