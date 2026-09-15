//! What generating a report costs.
//!
//! # Four renderings, four different jobs
//!
//! A report is rendered for a pipeline (JSON), a person (Markdown), a graph renderer
//! (DOT) and a test reporter (JUnit), and each is a different amount of work over the
//! same model. Benchmarking them together under one number would answer a question
//! nobody asks; benchmarking them apart is what shows which format a CI job should
//! choose when it renders on every commit.
//!
//! Two of the four are deliberate refusals on some reports. A report document has no
//! graph field, so DOT can only be rendered for a report that carries one in memory, and
//! an empty digraph would read as a finding about the topology rather than as its
//! absence. The refusal is measured too: it is a real path a caller takes.
//!
//! # Why the validation is inside the measurement
//!
//! Every renderer validates the report before writing it, because a document that
//! violates its own schema is worse than no document. That validation is part of the
//! cost of rendering, so it is measured with the rendering rather than factored out to
//! make the numbers look smaller.

use std::hint::black_box;

use amasario_integration_tests::documents::{self, ReportKind};
use amasario_report::formatter::{Format, render, render_summary};
use criterion::{Criterion, criterion_group, criterion_main};

/// Every rendering of every corpus report.
///
/// The three kinds are one report each of an observed fact, an unanswerable question and
/// a cyclic graph, so the four formats are measured over the three shapes of content the
/// engine writes rather than over one.
fn formats(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("report/render");
    for kind in ReportKind::all() {
        let report = documents::report(*kind);
        for format in [Format::Json, Format::Markdown, Format::Dot, Format::Junit] {
            group.bench_function(format!("{}/{format:?}", kind.file()), |bencher| {
                bencher.iter(|| {
                    // Either outcome is a real result; `expect` would hide the refusal
                    // path that a report without a graph takes.
                    black_box(render(black_box(&report), format).map(|out| out.len()).ok())
                });
            });
        }
    }
    group.finish();
}

/// The one-line summary, which is what a terminal shows before anything is written.
fn summary(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("report/summary");
    for kind in ReportKind::all() {
        let report = documents::report(*kind);
        group.bench_function(kind.file(), |bencher| {
            bencher.iter(|| black_box(render_summary(black_box(&report))));
        });
    }
    group.finish();
}

/// Rendering a report that grew, so the cost is a function of size rather than of shape.
///
/// A report's `observedFacts` is the section that grows with what was observed, so the
/// corpus report is extended with copies of its own statements. No new kind of content is
/// invented: the repeated statement is the one the fixture already asserts, which keeps
/// the benchmark measuring the renderer rather than the corpus.
fn wide(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("report/render/wide");
    for copies in [0_usize, 16, 256] {
        let mut report = documents::report(ReportKind::Verified);
        let template: Vec<_> = report.sections.observed_facts.clone();
        for _ in 0..copies {
            report
                .sections
                .observed_facts
                .extend(template.iter().cloned());
        }
        let facts = report.sections.observed_facts.len();

        group.bench_function(format!("{facts}-facts/json"), |bencher| {
            bencher.iter(|| {
                black_box(
                    render(black_box(&report), Format::Json)
                        .expect("a report renders as JSON")
                        .len(),
                )
            });
        });
        group.bench_function(format!("{facts}-facts/markdown"), |bencher| {
            bencher.iter(|| {
                black_box(
                    render(black_box(&report), Format::Markdown)
                        .expect("a report renders as Markdown")
                        .len(),
                )
            });
        });
    }
    group.finish();
}

criterion_group!(benches, formats, summary, wide);
criterion_main!(benches);
