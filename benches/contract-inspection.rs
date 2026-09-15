//! What contract inspection costs.
//!
//! # What is being measured, and why it is worth measuring
//!
//! Inspection is the first thing the engine does and the only step whose input arrives
//! from outside the engine. Every later stage - dependency discovery, graph assembly,
//! impact propagation - runs over data the engine derived itself, but the module decoder
//! runs over bytes an endpoint returned, so its behaviour on a hostile or merely
//! malformed input is a security-relevant property rather than only a performance one.
//!
//! The corpus is used deliberately: it holds two real modules, one module that is a
//! valid module with no interface, and two byte sequences that are not modules at all.
//! A benchmark measured over the valid modules alone would report a cost that a
//! truncated header never pays, because the interesting path is the refusal.
//!
//! # What the numbers are not
//!
//! These are not a service-level objective. The engine's cost on a real network is
//! dominated by the round trip and by the ledger boundary, and a benchmark that
//! pretended otherwise would invite a reader to optimise the wrong step.

use std::hint::black_box;

use amasario_contract::wasm;
use amasario_integration_tests::wasm_modules;
use criterion::{Criterion, criterion_group, criterion_main};

/// Decodes every module in the corpus, including the two that must be refused.
fn decode(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("inspection/parse-module");
    for spec in wasm_modules::all() {
        let bytes = (spec.bytes)();
        group.bench_function(spec.name, |bencher| {
            bencher.iter(|| {
                // Either outcome is a real result. `expect` would measure only the
                // modules that decode, and the refusal is the case worth timing.
                black_box(wasm::parse_module(black_box(&bytes)).is_ok())
            });
        });
    }
    group.finish();
}

/// Computes the module digest, which is the identity every provenance claim rests on.
///
/// Separated from decoding because it is a separate cost on a separate path: the
/// deployment's digest is computed over the module the same way regardless of whether
/// an interface could be decoded from it.
fn digest(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("inspection/digest");
    for spec in wasm_modules::all() {
        let bytes = (spec.bytes)();
        group.bench_function(spec.name, |bencher| {
            bencher.iter(|| black_box(wasm::digest_of(black_box(&bytes))));
        });
    }
    group.finish();
}

/// The cheap pre-check, which is asked before the decoder is paid for.
///
/// Benchmarked separately because its documented contract is to be cheaper than
/// [`wasm::parse_module`]; a benchmark that only measured the decoder could not show
/// that the pre-check earns its place.
fn precheck(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("inspection/looks-like-module");
    for spec in wasm_modules::all() {
        let bytes = (spec.bytes)();
        group.bench_function(spec.name, |bencher| {
            bencher.iter(|| black_box(wasm::looks_like_module(black_box(&bytes))));
        });
    }
    group.finish();
}

/// Verifying a recorded digest against the bytes is what makes a claim checkable.
///
/// The mismatch case is included on purpose. A comparison that short-circuits on its
/// first differing byte and one that hashes the whole module differ by orders of
/// magnitude on a mismatch, and a benchmark that only ever matched would hide which
/// of the two the engine does - which matters, because the wrong one is a timing
/// side channel against a value an attacker chooses.
fn verify(criterion: &mut Criterion) {
    let bytes = wasm_modules::custom_section_module();
    let honest = wasm::digest_of(&bytes);
    let wrong = amasario_integration_tests::documents::digest_of(0x00);

    let mut group = criterion.benchmark_group("inspection/verify-digest");
    group.bench_function("matches", |bencher| {
        bencher
            .iter(|| black_box(wasm::verify_digest(black_box(&honest), black_box(&bytes)).is_ok()));
    });
    group.bench_function("differs", |bencher| {
        bencher
            .iter(|| black_box(wasm::verify_digest(black_box(&wrong), black_box(&bytes)).is_ok()));
    });
    group.finish();
}

criterion_group!(benches, decode, digest, precheck, verify);
criterion_main!(benches);
