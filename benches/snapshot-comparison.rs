//! What capturing, storing and comparing a snapshot costs.
//!
//! # Why the comparison is the interesting one
//!
//! A snapshot is the only artefact in the engine that is meant to be kept: it is written
//! at one boundary and read again at another, so the comparison is what turns two
//! records into a finding. Its cost is not linear in the snapshot's size - every
//! collection is compared by identity, every edge by its derived identifier, and the
//! result is checked for duplicate identifiers before it is returned - so a benchmark
//! that measured serialisation and called it snapshot handling would miss the step
//! whose complexity actually grows.
//!
//! # The three costs, measured apart
//!
//! * **capture** - building a snapshot from the engine's own types, including the
//!   content digest computed over the canonical form.
//! * **store** - the JSON round trip, which is how a snapshot survives between runs.
//! * **compare** - the diff, over a pair the corpus designed to differ in the
//!   dependency surface, the module digest and the confidence at once.
//!
//! The `wide` cases matter more than the corpus pair. The corpus diff is deliberately
//! small enough to read; the wide cases repeat each collection so that the identity
//! comparisons are measured at a size the fixture deliberately does not reach.

use std::hint::black_box;

use amasario_integration_tests::documents::{self, evidence};
use amasario_snapshot::{Snapshot, compare, content_digest, from_json, to_json, to_json_pretty};
use criterion::{Criterion, criterion_group, criterion_main};

/// Capturing the corpus pair, including the content digest over each canonical form.
fn capture(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("snapshot/capture");
    group.bench_function("pair", |bencher| {
        bencher.iter(|| black_box(documents::snapshots()));
    });
    group.finish();
}

/// The JSON round trip, which is how a snapshot survives between runs.
fn store(criterion: &mut Criterion) {
    let (before, _) = documents::snapshots();
    let json = to_json(&before).expect("the snapshot serialises");
    let pretty = to_json_pretty(&before).expect("the snapshot serialises");

    let mut group = criterion.benchmark_group("snapshot/store");
    group.bench_function("to-json", |bencher| {
        bencher.iter(|| black_box(to_json(black_box(&before)).expect("serialises")));
    });
    group.bench_function("to-json-pretty", |bencher| {
        bencher.iter(|| black_box(to_json_pretty(black_box(&before)).expect("serialises")));
    });
    group.bench_function("from-json", |bencher| {
        bencher.iter(|| {
            let parsed: Snapshot =
                from_json(black_box(&json), "bench.json").expect("the document parses");
            black_box(parsed.evidence.len())
        });
    });
    group.bench_function("from-json-pretty", |bencher| {
        bencher.iter(|| {
            let parsed: Snapshot =
                from_json(black_box(&pretty), "bench.json").expect("the document parses");
            black_box(parsed.evidence.len())
        });
    });
    group.finish();
}

/// The diff over the corpus pair, which differs in three categories at once.
fn comparison(criterion: &mut Criterion) {
    let (before, after) = documents::snapshots();
    let mut group = criterion.benchmark_group("snapshot/compare");
    group.bench_function("corpus-pair", |bencher| {
        bencher.iter(|| {
            let diff = compare(
                black_box(&before),
                black_box(&after),
                "2026-01-02T00:00:00Z",
            )
            .expect("the pair is comparable");
            black_box(diff.changes.len())
        });
    });
    group.bench_function("identical-pair", |bencher| {
        bencher.iter(|| {
            // The shortest job, and worth measuring: it is the case a CI job runs on
            // every commit, where only the answer "nothing changed" is interesting.
            let diff = compare(
                black_box(&before),
                black_box(&before),
                "2026-01-02T00:00:00Z",
            )
            .expect("a snapshot is comparable with itself");
            black_box(diff.changes.len())
        });
    });
    group.finish();
}

/// Repeats a snapshot's evidence until it holds `copies` full rounds of the corpus.
///
/// The content digest is recomputed because the comparison verifies it: a snapshot whose
/// recorded digest disagreed with its contents would be refused before any comparison
/// happened, and the benchmark would silently be measuring the refusal.
fn widened(base: &Snapshot, copies: usize) -> Snapshot {
    let mut snapshot = base.clone();
    for copy in 0..copies {
        for mut record in evidence() {
            record.id = format!("{}-w{copy}", record.id);
            snapshot.evidence.push(record);
        }
    }
    snapshot.content_digest = content_digest(&snapshot).expect("a digest over the contents");
    snapshot
}

/// Comparison cost as a function of how much evidence there is to compare.
///
/// The corpus pair holds a handful of records, which cannot show scaling. Repeating them
/// is honest rather than synthetic in the way that matters: the comparison's job on an
/// added record does not depend on the record's contents.
fn wide(criterion: &mut Criterion) {
    let (before, _) = documents::snapshots();
    let mut group = criterion.benchmark_group("snapshot/compare/wide");
    for copies in [0_usize, 4, 64] {
        let earlier = widened(&before, copies);
        let later = {
            let mut snapshot = widened(&before, copies);
            // One record differs, so every collection still has work to do.
            snapshot.confidence.level = amasario_core::ConfidenceLevel::MediumConfidence;
            snapshot.content_digest =
                content_digest(&snapshot).expect("a digest over the contents");
            snapshot
        };
        let records = earlier.evidence.len();
        group.bench_function(format!("{records}-records"), |bencher| {
            bencher.iter(|| {
                let diff = compare(
                    black_box(&earlier),
                    black_box(&later),
                    "2026-01-02T00:00:00Z",
                )
                .expect("the pair is comparable");
                black_box(diff.changes.len())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, capture, store, comparison, wide);
criterion_main!(benches);
