//! Corpus-wide invariants: the checks that no single fixture can carry on its own.
//!
//! # What belongs in this suite
//!
//! Every other suite checks one family of documents against one model. This one checks
//! the properties that cross families, and that would therefore go unchecked if each
//! suite only looked at its own directory:
//!
//! * every document the generator writes is current, in one place, so a fixture added
//!   without a suite still cannot drift;
//! * every JSON file in the corpus parses, except the one file that exists to be
//!   unparseable and is named as such;
//! * every document that carries a specification version carries the engine's;
//! * the fuzz targets' invariant holds over a corpus of malformed input, which is how the
//!   fuzzing logic is exercised on every pull request rather than only in a nightly job;
//! * the corpus's own README describes the corpus that exists.

use std::collections::BTreeSet;

use amasario_integration_tests::corpus::{Corpus, assert_committed};
use amasario_integration_tests::corpus_readme;
use amasario_integration_tests::documents::{
    self, GraphKind, ImpactKind, ProvenanceKind, ReportKind, SetKind,
};
use amasario_integration_tests::harness;
use amasario_integration_tests::recordings;

/// The fixture directories the repository structure names, in sorted order.
///
/// `reference` is the only one whose contents are not produced by
/// `generate-fixtures`: it holds the modules this repository's own contracts build, with
/// a provenance record beside each, and its records are checked against those modules
/// rather than rebuilt from a model.
const DIRECTORIES: [&str; 11] = [
    "contracts",
    "dependencies",
    "expected-reports",
    "graphs",
    "impact",
    "ledgers",
    "provenance",
    "reference",
    "snapshots",
    "transactions",
    "wasm",
];

/// Every document the generator writes is exactly what the builders produce.
///
/// One assertion per family, in one test, so that a fixture added without a suite of its
/// own is still held to the same standard. The per-family suites check more; this checks
/// that nothing escaped them.
#[test]
fn every_generated_document_is_current() {
    for kind in SetKind::all() {
        assert_committed(
            "dependencies",
            &format!("{}.json", kind.file()),
            &documents::rendered(&documents::set_document(*kind)),
        );
    }
    for kind in GraphKind::all() {
        assert_committed(
            "graphs",
            &format!("{}.json", kind.file()),
            &documents::rendered(&documents::graph_document(*kind)),
        );
    }
    for kind in ProvenanceKind::all() {
        assert_committed(
            "provenance",
            &format!("{}.json", kind.file()),
            &documents::rendered(&documents::provenance_chain(*kind)),
        );
    }
    for kind in ImpactKind::all() {
        assert_committed(
            "impact",
            &format!("{}.json", kind.file()),
            &documents::rendered(&documents::impact_analysis(*kind).findings),
        );
    }
    for kind in ReportKind::all() {
        assert_committed(
            "expected-reports",
            &format!("{}.json", kind.file()),
            &documents::rendered(&documents::report(*kind)),
        );
    }
    for recording in recordings::all() {
        assert_committed(recording.directory, recording.file, &recording.rendered());
    }
    for (name, identity) in documents::contract_identities() {
        assert_committed(
            "contracts",
            &format!("{name}.json"),
            &documents::rendered(&identity),
        );
    }
    let (before, after) = documents::snapshots();
    assert_committed(
        "snapshots",
        "testnet-alpha-before.json",
        &documents::rendered(&before),
    );
    assert_committed(
        "snapshots",
        "testnet-alpha-after.json",
        &documents::rendered(&after),
    );
}

/// The corpus has no fixture directory the structure does not name, and no missing one.
#[test]
fn the_corpus_holds_exactly_the_directories_the_structure_names() {
    let root = Corpus::root();
    let entries = std::fs::read_dir(&root).expect("the corpus root is readable");
    let mut directories: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    directories.sort();

    let expected: Vec<String> = DIRECTORIES.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        directories, expected,
        "the fixture corpus and the repository structure disagree"
    );
}

/// Every JSON file in the corpus parses, except the one that exists to be unparseable.
///
/// The exception is by name rather than by directory, because a per-directory exemption
/// would hide a genuinely broken file in the same directory later.
#[test]
fn every_fixture_parses_except_the_one_that_exists_not_to() {
    let unparseable = ["malformed-response.json"];

    let mut checked = 0usize;
    for directory in DIRECTORIES {
        for path in Corpus::files_in(directory) {
            let name = path
                .file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path).expect("the fixture is readable");

            if unparseable.contains(&name.as_str()) {
                assert!(
                    serde_json::from_str::<serde_json::Value>(&text).is_err(),
                    "fixtures/{directory}/{name} is exempt from parsing because it is the \
                     corpus's malformed recording, so it must actually be malformed"
                );
                continue;
            }

            serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|error| {
                panic!("fixtures/{directory}/{name} is not valid JSON: {error}")
            });
            checked += 1;
        }
    }

    assert!(
        checked > 30,
        "the corpus should hold a substantial number of readable fixtures, checked {checked}"
    );
}

/// Every document that records a specification version records the engine's.
///
/// A fixture from another specification version is a document the engine would refuse to
/// interpret, so a corpus containing one would be a corpus half of which is unusable.
#[test]
fn every_versioned_document_names_the_version_the_engine_supports() {
    let mut versions: BTreeSet<String> = BTreeSet::new();
    let mut documents_with_versions = 0usize;

    for directory in DIRECTORIES {
        for path in Corpus::files_in(directory) {
            let text = std::fs::read_to_string(&path).expect("readable");
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(object) = value.as_object() else {
                // A finding set is a JSON array rather than a document envelope.
                continue;
            };
            if let Some(version) = object.get("specVersion").and_then(|value| value.as_str()) {
                versions.insert(version.to_owned());
                documents_with_versions += 1;
            }
        }
    }

    assert!(
        documents_with_versions > 0,
        "the corpus must hold at least one versioned document"
    );
    assert_eq!(
        versions.len(),
        1,
        "the corpus holds documents from {} specification versions: {versions:?}",
        versions.len()
    );
}

/// The fuzz targets' invariant holds over a corpus of malformed input.
///
/// This is how the fuzzing logic is exercised on every pull request. The targets under
/// `fuzz/` need a nightly toolchain and run on a schedule; the bodies they call live in
/// `amasario_integration_tests::harness` and are run here, so a change that made a
/// malformed input panic or hang would fail `CI` rather than waiting for the next nightly
/// fuzz run to notice.
#[test]
fn a_malformed_input_is_classified_rather_than_panicking() {
    let checked = harness::check_invariant();
    let expected = harness::Harness::all().len() * harness::malformed_corpus().len();
    assert_eq!(checked, expected, "every harness ran over every input");

    // And a *valid* document is accepted, so the invariant is not satisfied by refusing
    // everything - which would be the degenerate way to pass the check above.
    let valid = harness::valid_dependency_bytes();
    let outcome = harness::Harness::Dependencies.run(&valid);
    assert!(
        outcome.is_accepted(),
        "a document the builder produced must be accepted: {}",
        outcome.description()
    );

    let refused = harness::Harness::Dependencies.run(b"{");
    assert!(refused.is_refused());
    assert!(!refused.description().is_empty());
}

/// The corpus's README describes the corpus that exists.
#[test]
fn the_corpus_readme_is_current() {
    let path = Corpus::root().join("README.md");
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
    assert_eq!(
        committed,
        corpus_readme::render(),
        "fixtures/README.md no longer describes the corpus; regenerate with \
         `cargo run -p amasario-integration-tests --bin generate-fixtures`"
    );
}

/// No fixture directory is empty.
///
/// An empty directory is the shape a surface takes when it was created and then not
/// built, and it is the case the workflows deliberately distinguish from an absent one.
/// The corpus must not contain any.
#[test]
fn no_fixture_directory_is_empty() {
    for directory in DIRECTORIES {
        let files = Corpus::files_in(directory);
        assert!(
            !files.is_empty(),
            "fixtures/{directory} holds no JSON fixture"
        );
    }
}

/// Every identifier a document cites is non-empty.
///
/// A citation that is an empty string is a citation of nothing: it satisfies "has
/// evidence" while naming none, which is the failure mode the evidence requirement
/// exists to prevent.
#[test]
fn no_document_cites_an_empty_identifier() {
    let mut citations = 0usize;
    for directory in DIRECTORIES {
        for path in Corpus::files_in(directory) {
            let text = std::fs::read_to_string(&path).expect("readable");
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            for citation in collect_citations(&value) {
                assert!(
                    !citation.trim().is_empty(),
                    "{} cites an empty identifier",
                    path.display()
                );
                citations += 1;
            }
        }
    }
    assert!(citations > 0, "the corpus cites evidence somewhere");
}

/// Every string that sits under an `evidence` key, wherever it is in a document.
///
/// Written as a walk rather than as a typed read because the point is to catch a
/// citation in *any* document shape, including one added later that no type here knows
/// about. A typed read would only check the shapes this test was written against.
fn collect_citations(value: &serde_json::Value) -> Vec<String> {
    fn walk(value: &serde_json::Value, key: Option<&str>, out: &mut Vec<String>) {
        if key == Some("evidence") {
            match value {
                serde_json::Value::Array(items) => {
                    for item in items {
                        match item {
                            serde_json::Value::String(text) => out.push(text.clone()),
                            serde_json::Value::Object(object) => {
                                // An evidence reference is an object with a kind and an
                                // identifier, so the identifier is what is checked.
                                if let Some(id) =
                                    object.get("id").and_then(serde_json::Value::as_str)
                                {
                                    out.push(id.to_owned());
                                }
                            },
                            _ => {},
                        }
                    }
                },
                serde_json::Value::String(text) => out.push(text.clone()),
                _ => {},
            }
        }

        match value {
            serde_json::Value::Object(object) => {
                for (name, child) in object {
                    walk(child, Some(name.as_str()), out);
                }
            },
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, key, out);
                }
            },
            _ => {},
        }
    }

    let mut out = Vec::new();
    walk(value, None, &mut out);
    out
}
