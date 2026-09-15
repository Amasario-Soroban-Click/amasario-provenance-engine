//! The invariant every fuzz target checks, in one place and runnable without a fuzzer.
//!
//! # The invariant
//!
//! A malformed input produces a classified refusal, never a panic and never an
//! unbounded loop. That is one sentence, and it is the whole contract: an analysis
//! tool reads bytes it did not write - a hostile or merely broken RPC response, a
//! hand-edited snapshot, a truncated module - and the failure mode that matters is
//! the one where it dies or hangs instead of saying what was wrong.
//!
//! # Why the bodies live here rather than only in `fuzz/`
//!
//! A fuzz target is a `cargo fuzz` binary, which needs a nightly toolchain and a
//! `libfuzzer` build. If the logic lived only there it would be compiled by the fuzz
//! workflow and by nothing else, so a pull request could break it without any job
//! noticing. Keeping the bodies here means an ordinary test runs every one of them
//! over a corpus of malformed inputs, and the targets under `fuzz/` are thin wrappers
//! that call the same functions.

use amasario_dependency::DependencySet;
use amasario_dependency::document::DependencySetDocument;
use amasario_graph::GraphDocument;
use amasario_impact::ImpactFinding;
use amasario_snapshot::store;

/// What a harness did with its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The input was accepted, and the description says what was built from it.
    Accepted(String),
    /// The input was refused, and the description says why.
    Refused(String),
}

impl Outcome {
    /// Whether the input was refused.
    #[must_use]
    pub const fn is_refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }

    /// Whether the input was accepted.
    #[must_use]
    pub const fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted(_))
    }

    /// The description, whichever variant this is.
    #[must_use]
    pub fn description(&self) -> &str {
        match self {
            Self::Accepted(detail) | Self::Refused(detail) => detail,
        }
    }
}

/// The name of a harness, so that the fuzz targets and the tests address one registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    /// A dependency set document, in the specification's shape.
    Dependencies,
    /// A dependency graph document.
    Graph,
    /// A recorded snapshot.
    Snapshot,
    /// A set of impact findings.
    Impact,
    /// A WebAssembly module.
    Wasm,
    /// A relative link from `error.schema.json`'s vocabulary, which is the smallest
    /// input the engine classifies.
    ProvenanceRecord,
}

impl Harness {
    /// Every harness, which is also the list of fuzz targets.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Dependencies,
            Self::Graph,
            Self::Snapshot,
            Self::Impact,
            Self::Wasm,
            Self::ProvenanceRecord,
        ]
    }

    /// The target's name, used as a `cargo fuzz` target and as a test case name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dependencies => "dependency-fuzzer",
            Self::Graph => "graph-fuzzer",
            Self::Snapshot => "snapshot-fuzzer",
            Self::Impact => "impact-fuzzer",
            Self::Wasm => "wasm-fuzzer",
            Self::ProvenanceRecord => "provenance-fuzzer",
        }
    }

    /// Runs this harness over an input.
    ///
    /// This function must not panic for any input. Every branch below returns an
    /// [`Outcome`]; there is no `unwrap`, no indexing and no recursion.
    #[must_use]
    pub fn run(self, input: &[u8]) -> Outcome {
        match self {
            Self::Dependencies => dependencies(input),
            Self::Graph => graph(input),
            Self::Snapshot => snapshot(input),
            Self::Impact => impact(input),
            Self::Wasm => wasm(input),
            Self::ProvenanceRecord => provenance_record(input),
        }
    }
}

/// The text of an input, or a refusal when it is not UTF-8.
fn as_text(input: &[u8]) -> Result<&str, Outcome> {
    std::str::from_utf8(input).map_err(|error| {
        Outcome::Refused(format!(
            "the input is not UTF-8 at byte {}: {error}",
            error.valid_up_to()
        ))
    })
}

/// Parses a dependency set document and checks its invariants.
#[must_use]
pub fn dependencies(input: &[u8]) -> Outcome {
    let text = match as_text(input) {
        Ok(text) => text,
        Err(outcome) => return outcome,
    };
    match serde_json::from_str::<DependencySetDocument>(text) {
        Ok(document) => {
            let violations = document.partition_violations();
            if violations.is_empty() {
                Outcome::Accepted(format!(
                    "a dependency set document with {} edge(s)",
                    document.edges.len()
                ))
            } else {
                Outcome::Refused(format!(
                    "the document parsed and violates the partition rule: {}",
                    violations.join("; ")
                ))
            }
        },
        Err(error) => Outcome::Refused(format!("the document did not parse: {error}")),
    }
}

/// Parses a graph document and checks its compatibility and integrity.
#[must_use]
pub fn graph(input: &[u8]) -> Outcome {
    let text = match as_text(input) {
        Ok(text) => text,
        Err(outcome) => return outcome,
    };
    match GraphDocument::from_json(text) {
        Ok(document) => {
            let unresolvable = document.unresolvable_endpoints().len();
            if unresolvable == 0 {
                Outcome::Accepted(format!(
                    "a graph document with {} node(s) and {} edge(s)",
                    document.nodes.len(),
                    document.edges.len()
                ))
            } else {
                Outcome::Refused(format!(
                    "{unresolvable} edge endpoint(s) name a node the document does not define"
                ))
            }
        },
        Err(error) => Outcome::Refused(format!("the document did not parse: {error}")),
    }
}

/// Parses a snapshot the way the store does, including its digest check.
#[must_use]
pub fn snapshot(input: &[u8]) -> Outcome {
    let text = match as_text(input) {
        Ok(text) => text,
        Err(outcome) => return outcome,
    };
    match store::from_json(text, "<fuzz>") {
        Ok(snapshot) => {
            let failures = snapshot.failures();
            if failures.is_empty() {
                Outcome::Accepted(format!(
                    "a snapshot with {} evidence record(s)",
                    snapshot.evidence.len()
                ))
            } else {
                Outcome::Refused(format!(
                    "the snapshot parsed and failed {} check(s): {}",
                    failures.len(),
                    failures
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ))
            }
        },
        Err(error) => Outcome::Refused(format!("the snapshot did not parse: {error}")),
    }
}

/// Parses a set of impact findings.
#[must_use]
pub fn impact(input: &[u8]) -> Outcome {
    let text = match as_text(input) {
        Ok(text) => text,
        Err(outcome) => return outcome,
    };
    match serde_json::from_str::<Vec<ImpactFinding>>(text) {
        Ok(findings) => {
            let failures: Vec<String> = findings
                .iter()
                .flat_map(|finding| finding.failures())
                .map(|failure| failure.to_string())
                .collect();
            if failures.is_empty() {
                Outcome::Accepted(format!("a finding set with {} finding(s)", findings.len()))
            } else {
                Outcome::Refused(format!(
                    "{} finding(s) violate a rule: {}",
                    failures.len(),
                    failures.join("; ")
                ))
            }
        },
        Err(error) => Outcome::Refused(format!("the findings did not parse: {error}")),
    }
}

/// Parses a WebAssembly module.
#[must_use]
pub fn wasm(input: &[u8]) -> Outcome {
    if !amasario_contract::wasm::looks_like_module(input) {
        return Outcome::Refused(format!(
            "the first bytes are not a WebAssembly header, so this is not a module at all \
             ({} byte(s) given)",
            input.len()
        ));
    }
    match amasario_contract::wasm::parse_module(input) {
        Ok(module) => Outcome::Accepted(format!(
            "a module with {} section(s) and {} import(s)",
            module.sections.len(),
            module.imports.len()
        )),
        Err(error) => Outcome::Refused(format!("the module did not decode: {error}")),
    }
}

/// Classifies a claimed-provenance record.
///
/// The smallest harness, kept because it covers the one path a fuzzer can reach
/// without a document: a digest, a verification status and a relationship name, each
/// of which is compared against a closed vocabulary before it is used.
#[must_use]
pub fn provenance_record(input: &[u8]) -> Outcome {
    let text = match as_text(input) {
        Ok(text) => text,
        Err(outcome) => return outcome,
    };
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => return Outcome::Refused(format!("the record did not parse: {error}")),
    };

    let Some(object) = value.as_object() else {
        return Outcome::Refused("the record is not a JSON object".to_owned());
    };

    let mut established: Vec<&str> = Vec::new();

    if let Some(digest) = object.get("digest").and_then(serde_json::Value::as_str) {
        if let Err(error) =
            amasario_core::Digest::new(amasario_core::DigestAlgorithm::Sha256, digest)
        {
            return Outcome::Refused(format!("the digest is not a SHA-256 value: {error}"));
        }
        established.push("a digest");
    }

    if let Some(status) = object
        .get("verification")
        .and_then(serde_json::Value::as_str)
    {
        if let Err(error) = status.parse::<amasario_core::VerificationStatus>() {
            return Outcome::Refused(format!(
                "the verification status is not one of the five: {error}"
            ));
        }
        established.push("a verification status");
    }

    if let Some(relationship) = object
        .get("relationship")
        .and_then(serde_json::Value::as_str)
    {
        if let Err(error) = relationship.parse::<amasario_core::Relationship>() {
            return Outcome::Refused(format!("the relationship is not one of the eight: {error}"));
        }
        established.push("a relationship");
    }

    Outcome::Accepted(format!(
        "a record carrying {}",
        if established.is_empty() {
            "nothing the engine can act on".to_owned()
        } else {
            established.join(", ")
        }
    ))
}

/// The inputs the invariant test runs every harness over.
///
/// Not a corpus in the fuzzing sense: these are the shapes a document reader is most
/// likely to be handed, written down so that the invariant is checked on every test
/// run rather than only in a nightly fuzz job.
#[must_use]
pub fn malformed_corpus() -> Vec<&'static [u8]> {
    vec![
        b"",
        b"{",
        b"[]",
        b"null",
        b"{}",
        b"{\"edges\": []}",
        b"{\"nodes\": [], \"edges\": [{\"id\": \"e\"}]}",
        b"{\"apiVersion\": \"amasario.dev/v1\", \"specVersion\": \"1.0.0\", \"nodes\": [{\"id\": \"n\", \"kind\": \"CONTRACT\"}], \"edges\": [{\"id\": \"e\", \"source\": \"n\", \"target\": \"missing\", \"relationship\": \"DEPENDS_ON\", \"evidence\": [], \"confidence\": {}, \"observed\": true}]}",
        b"{\"nested\": {\"deeply\": {\"deeper\": [1, 2, 3, {\"x\": null}]}}}",
        b"\xff\xfe\x00\x01",
        b"\x00asm",
        b"\x00asm\x02\x00\x00\x00",
        b"\x00asm\x01\x00\x00\x00\x00\xff\xff\xff\xff",
        &[0u8; 4096],
        &[0xffu8; 4096],
    ]
}

/// Every harness, over every malformed input, asserting the invariant.
///
/// # Returns
///
/// The number of (harness, input) pairs that were checked, so that a caller can assert
/// the corpus actually ran rather than having been empty.
#[must_use]
pub fn check_invariant() -> usize {
    let mut checked = 0;
    for harness in Harness::all() {
        for input in malformed_corpus() {
            // The call itself is the assertion: a panic fails the test, and there is no
            // error return to swallow. The invariant is that a bad input is classified.
            let outcome = harness.run(input);
            assert!(
                !outcome.description().is_empty(),
                "{harness:?} produced an outcome with no explanation for a {}-byte input",
                input.len()
            );
            checked += 1;
        }
    }
    checked
}

/// A dependency set, for a caller that wants to fuzz a valid document's bytes.
///
/// # Panics
///
/// Panics when the corpus set cannot be built, which is a defect in the corpus.
#[must_use]
pub fn valid_dependency_bytes() -> Vec<u8> {
    let set: DependencySet = crate::documents::set(crate::documents::SetKind::Direct);
    let document = DependencySetDocument::of(&set).expect("the set projects");
    serde_json::to_vec(&document).expect("a document serialises")
}
