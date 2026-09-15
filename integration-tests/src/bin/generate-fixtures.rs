//! Writes the whole fixture corpus.
//!
//! Run with `cargo run -p amasario-integration-tests --bin generate-fixtures`.
//!
//! # Why this is a program rather than a build script
//!
//! A build script runs on every build and writes into `OUT_DIR`, so a fixture could
//! never be reviewed in a diff. These are committed files that a reviewer reads, which
//! means the write has to be an explicit act. Every suite asserts that what is committed
//! equals what the builders produce, so the explicit act is also a checked one:
//! forgetting to regenerate fails a test rather than producing a stale corpus.
//!
//! The program is idempotent. Running it twice writes the same bytes, and `--check`
//! writes nothing and reports whether the corpus is current, which is what a workflow
//! wants when it only needs to know.

use std::fs;

use amasario_integration_tests::corpus::Corpus;
use amasario_integration_tests::corpus_readme;
use amasario_integration_tests::documents::{
    self, GraphKind, ImpactKind, ProvenanceKind, ReportKind, SetKind, rendered,
};
use amasario_integration_tests::recordings;
use amasario_integration_tests::wasm_modules;

// `std::process::exit` is disallowed so that library code stays testable and the exit
// code is decided in one place. This program *is* that place: it is a binary with no
// library callers, and returning a code through `main` is the whole of its contract.
#[allow(
    clippy::disallowed_methods,
    reason = "this binary is the entry point the rule reserves the exit code for; it has no library callers"
)]
fn main() {
    let check_only = std::env::args().any(|argument| argument == "--check");
    let root = Corpus::root();

    let mut written = 0usize;
    let mut stale: Vec<String> = Vec::new();

    // One emitter for the whole run, so that `--check` and a write take the same path
    // through the corpus and cannot disagree about what is expected.
    let mut emit = |directory: &str, file: &str, contents: String| {
        let relative = if directory.is_empty() {
            format!("fixtures/{file}")
        } else {
            Corpus::relative(directory, file)
        };
        let path = if directory.is_empty() {
            root.join(file)
        } else {
            root.join(directory).join(file)
        };

        if check_only {
            match fs::read_to_string(&path) {
                Ok(existing) if existing == contents => {},
                Ok(_) => stale.push(relative),
                Err(error) => stale.push(format!("{relative} ({error})")),
            }
            return;
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                fail(&format!(
                    "{} could not be created: {error}",
                    parent.display()
                ));
            });
        }
        fs::write(&path, contents).unwrap_or_else(|error| {
            fail(&format!("{} could not be written: {error}", path.display()));
        });
        written += 1;
    };

    // -- dependencies ---------------------------------------------------------
    for kind in SetKind::all() {
        emit(
            "dependencies",
            &format!("{}.json", kind.file()),
            rendered(&documents::set_document(*kind)),
        );
    }

    // -- graphs ---------------------------------------------------------------
    for kind in GraphKind::all() {
        emit(
            "graphs",
            &format!("{}.json", kind.file()),
            rendered(&documents::graph_document(*kind)),
        );
    }

    // -- provenance -----------------------------------------------------------
    for kind in ProvenanceKind::all() {
        emit(
            "provenance",
            &format!("{}.json", kind.file()),
            rendered(&documents::provenance_chain(*kind)),
        );
    }

    // -- impact ---------------------------------------------------------------
    //
    // The fixture is the finding set, which is the serialisable half of an analysis:
    // `ImpactAnalysis` carries the traversal's bounds and counters, which describe the
    // run rather than the result, and the changed entity is recorded in the corpus
    // README beside the count. Writing a synthetic wrapper object would invent a
    // document shape the specification does not define, which is the one thing a
    // fixture must never do.
    for kind in ImpactKind::all() {
        emit(
            "impact",
            &format!("{}.json", kind.file()),
            rendered(&documents::impact_analysis(*kind).findings),
        );
    }

    // -- snapshots ------------------------------------------------------------
    let (before, after) = documents::snapshots();
    emit("snapshots", "testnet-alpha-before.json", rendered(&before));
    emit("snapshots", "testnet-alpha-after.json", rendered(&after));

    // -- contracts ------------------------------------------------------------
    for (name, identity) in documents::contract_identities() {
        emit("contracts", &format!("{name}.json"), rendered(&identity));
    }

    // -- wasm -----------------------------------------------------------------
    for spec in wasm_modules::all() {
        let bytes = (spec.bytes)();
        let digest = amasario_contract::wasm::digest_of(&bytes);
        // The digest and the flags are computed here rather than transcribed, so a
        // fixture cannot claim a digest its bytes do not have. That is what makes the
        // module fixtures usable as an oracle: the suite recomputes both and compares.
        let document = serde_json::json!({
            "name": spec.name,
            "note": spec.note,
            "hex": wasm_modules::hex_of(&bytes),
            "byteSize": bytes.len(),
            "digest": digest.value(),
            "digestAlgorithm": "sha256",
            "magicPresent": spec.magic_present,
            "isAModule": spec.is_a_module,
        });
        let mut text = serde_json::to_string_pretty(&document).expect("a value serialises");
        text.push('\n');
        emit("wasm", &format!("{}.json", spec.name), text);
    }

    // -- recorded endpoint responses ------------------------------------------
    for recording in recordings::all() {
        emit(recording.directory, recording.file, recording.rendered());
    }

    // -- expected reports -----------------------------------------------------
    for kind in ReportKind::all() {
        emit(
            "expected-reports",
            &format!("{}.json", kind.file()),
            rendered(&documents::report(*kind)),
        );
    }

    // -- the corpus's own index ----------------------------------------------
    emit("", "README.md", corpus_readme::render());

    if check_only {
        if stale.is_empty() {
            println!("the fixture corpus is current");
            return;
        }
        eprintln!("the fixture corpus is stale:");
        for path in &stale {
            eprintln!("  {path}");
        }
        eprintln!(
            "Run `cargo run -p amasario-integration-tests --bin generate-fixtures` and review \
             the diff."
        );
        std::process::exit(1);
    }

    println!("wrote {written} fixture file(s) under {}", root.display());
}

/// Reports a failure and ends the run with a non-zero status.
///
/// `!` rather than `Result` because every caller is already unwinding a failed write:
/// a corpus generator that continued past a write error would report success for a
/// corpus it did not write.
#[allow(
    clippy::disallowed_methods,
    reason = "the only caller of this helper is the binary's entry point, which is where the exit code is decided"
)]
fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
