//! Cross-checks the engine's enumerated vocabularies against the normative
//! specification.
//!
//! # Why this test exists
//!
//! `amasario-provenance-spec` defines what the data means; this engine consumes it.
//! The engine therefore restates a number of the specification's enumerations as
//! Rust types - failure categories, entity kinds, relationship names, confidence
//! levels and so on - and a restatement can drift. Nothing in either repository
//! would notice if a term were renamed on one side and not the other: the
//! specification's schemas would still compile, and so would the engine. The
//! divergence would surface as a consumer whose report could not be parsed, which is
//! a late and confusing way to discover it.
//!
//! This test closes that gap by reading the specification's own files and comparing
//! them against the engine's types. It is the mirror image of the specification
//! repository's own checks, and it is the mechanism behind the requirement that the
//! engine consumes the specification rather than silently redefining it.
//!
//! # Running it
//!
//! The test needs the specification repository, so it is ignored by default: a test
//! suite that fails without an unrelated checkout is one people stop running. CI
//! checks the specification out beside the engine and runs it explicitly.
//!
//! ```console
//! # with a sibling checkout, which is the layout CI uses
//! cargo test -p amasario-core -- --include-ignored
//!
//! # or by pointing at one directly
//! AMASARIO_SPEC_DIR=/path/to/amasario-provenance-spec \
//!   cargo test -p amasario-core -- --include-ignored
//! ```
//!
//! # Comparisons are made as sets
//!
//! Where the specification lists terms in one order and the engine lists them in
//! another - `CONFLICTING` first among verification statuses, because it takes
//! precedence when two are combined - the comparison is between sets, not sequences.
//! Order is a matter for each artefact's own presentation; membership is the fact
//! that has to agree.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use amasario_core::{
    ConfidenceLevel, DependencyClass, EntityKind, ErrorCategory, EvidenceType, NetworkType,
    Relationship, TruncationReason, VerificationStatus,
};
use serde::Deserialize;

/// The minimum shape of a taxonomy document.
#[derive(Debug, Deserialize)]
struct Taxonomy {
    id: String,
    terms: Vec<Term>,
}

/// The minimum shape of a taxonomy term.
#[derive(Debug, Deserialize)]
struct Term {
    id: String,
}

/// Locates the specification repository.
///
/// Prefers an explicit path so that a checkout anywhere can be used, and otherwise
/// assumes the layout both repositories use in CI and in a local workspace: two
/// sibling directories.
fn specification_root() -> PathBuf {
    if let Ok(dir) = std::env::var("AMASARIO_SPEC_DIR") {
        let path = PathBuf::from(&dir);
        assert!(
            path.is_dir(),
            "AMASARIO_SPEC_DIR is set to {dir}, which is not a directory"
        );
        return path;
    }

    // <workspace>/amasario-provenance-engine/crates/amasario-core -> <workspace>
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("amasario-provenance-spec");

    assert!(
        sibling.is_dir(),
        "the normative specification was not found at {}. Check out \
         amasario-provenance-spec beside this repository, or set AMASARIO_SPEC_DIR. This check is \
         not optional: the engine's claim to consume the specification is only meaningful if \
         something verifies it.",
        sibling.display()
    );

    sibling
}

/// Reads the term identifiers of a taxonomy.
///
/// Panics rather than returning nothing when the file is missing or malformed,
/// because an absent file would otherwise make the comparison trivially pass against
/// an empty set - which is the opposite of what this test is for.
fn taxonomy_terms(root: &Path, name: &str) -> BTreeSet<String> {
    let path = root.join("taxonomies").join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    let taxonomy: Taxonomy = serde_norway::from_str(&text)
        .unwrap_or_else(|error| panic!("could not parse {}: {error}", path.display()));

    assert_eq!(
        taxonomy.id,
        name,
        "{} declares a different taxonomy id than its filename",
        path.display()
    );
    assert!(
        !taxonomy.terms.is_empty(),
        "{} declares no terms, so comparing against it would prove nothing",
        path.display()
    );

    taxonomy.terms.into_iter().map(|term| term.id).collect()
}

/// Reads the `enum` of a property from a JSON schema, following one `$ref` into a
/// `$defs` entry when the property is a reference.
fn schema_enum(root: &Path, file: &str, pointer: &[&str]) -> BTreeSet<String> {
    let path = root.join("schema").join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    let document: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("could not parse {}: {error}", path.display()));

    let mut current = &document;
    for segment in pointer {
        current = current
            .get(segment)
            .unwrap_or_else(|| panic!("{} has no {segment:?} at {pointer:?}", path.display()));
    }

    // A `$ref` is resolved one level, which is all the specification uses for the
    // enumerations compared here.
    if let Some(reference) = current.get("$ref").and_then(serde_json::Value::as_str) {
        let target = reference
            .rsplit('/')
            .next()
            .expect("a reference has a tail");
        current = document
            .get("$defs")
            .and_then(|defs| defs.get(target))
            .unwrap_or_else(|| panic!("{} has no $defs/{target}", path.display()));
    }

    let values = current
        .get("enum")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "{} enumeration at {pointer:?} is not a list",
                path.display()
            )
        });

    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("{} enumeration contains a non-string", path.display()))
                .to_owned()
        })
        .collect()
}

/// The engine's own names for a set of enumerated values.
fn engine_names(values: impl IntoIterator<Item = &'static str>) -> BTreeSet<String> {
    let names: BTreeSet<String> = values.into_iter().map(str::to_owned).collect();
    assert!(
        !names.is_empty(),
        "the engine enumerated nothing, which would make the comparison vacuous"
    );
    names
}

/// Asserts that two enumerations have the same members, naming the difference.
fn assert_same_members(label: &str, specification: &BTreeSet<String>, engine: &BTreeSet<String>) {
    let only_in_specification: Vec<&String> = specification.difference(engine).collect();
    let only_in_engine: Vec<&String> = engine.difference(specification).collect();

    assert!(
        only_in_specification.is_empty() && only_in_engine.is_empty(),
        "{label} diverged from the specification.\n  \
         declared by the specification but not by the engine: {only_in_specification:?}\n  \
         declared by the engine but not by the specification: {only_in_engine:?}\n  \
         A term removed on one side and not the other is a breaking change that neither \
         repository's own checks would notice."
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_error_categories_match_the_published_schema() {
    let root = specification_root();
    let specification = schema_enum(&root, "error.schema.json", &["properties", "category"]);

    assert_same_members(
        "ErrorCategory",
        &specification,
        &engine_names(ErrorCategory::all().iter().map(|c| c.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_entity_kinds_match_the_published_schema() {
    let root = specification_root();
    let specification = schema_enum(&root, "provenance.schema.json", &["$defs", "entityKind"]);

    assert_same_members(
        "EntityKind",
        &specification,
        &engine_names(EntityKind::all().iter().map(|k| k.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_relationship_types_match_the_taxonomy() {
    let root = specification_root();
    assert_same_members(
        "Relationship",
        &taxonomy_terms(&root, "relationship-types"),
        &engine_names(Relationship::all().iter().map(|r| r.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_dependency_classes_match_the_taxonomy() {
    let root = specification_root();
    assert_same_members(
        "DependencyClass",
        &taxonomy_terms(&root, "dependency-types"),
        &engine_names(DependencyClass::all().iter().map(|c| c.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_evidence_types_match_the_taxonomy() {
    let root = specification_root();
    assert_same_members(
        "EvidenceType",
        &taxonomy_terms(&root, "evidence-types"),
        &engine_names(EvidenceType::all().iter().map(|t| t.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_confidence_levels_match_the_taxonomy() {
    let root = specification_root();
    assert_same_members(
        "ConfidenceLevel",
        &taxonomy_terms(&root, "confidence-levels"),
        &engine_names(
            ConfidenceLevel::all_weakest_first()
                .iter()
                .map(|l| l.as_str()),
        ),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_verification_statuses_match_the_taxonomy() {
    let root = specification_root();
    assert_same_members(
        "VerificationStatus",
        &taxonomy_terms(&root, "verification-statuses"),
        &engine_names(VerificationStatus::all().iter().map(|s| s.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn the_network_types_match_the_taxonomy() {
    let root = specification_root();
    assert_same_members(
        "NetworkType",
        &taxonomy_terms(&root, "network-types"),
        &engine_names(NetworkType::all().iter().map(|t| t.as_str())),
    );
}

#[test]
#[ignore = "requires the amasario-provenance-spec checkout; CI provides it and runs with --include-ignored"]
fn every_taxonomy_the_engine_restates_is_present_and_readable() {
    // A guard against the failure mode where a taxonomy is renamed in the
    // specification and every comparison above silently finds an empty set. Each
    // reader asserts a non-empty result, so this test states the requirement
    // directly rather than relying on that assertion being noticed.
    let root = specification_root();
    for name in [
        "relationship-types",
        "dependency-types",
        "evidence-types",
        "confidence-levels",
        "verification-statuses",
        "network-types",
        "artifact-types",
        "impact-types",
        "deployment-statuses",
        "change-types",
    ] {
        let terms = taxonomy_terms(&root, name);
        assert!(!terms.is_empty(), "{name} declared no terms");
    }

    // `TruncationReason` has no taxonomy of its own: the specification defines
    // truncation in the provenance model rather than as a controlled vocabulary. The
    // engine therefore asserts only that its own values are stable strings, since
    // there is nothing authoritative to compare them against.
    for reason in [
        TruncationReason::MaxDepthReached,
        TruncationReason::MaxNodesReached,
        TruncationReason::EvidenceUnavailable,
        TruncationReason::BoundaryReached,
        TruncationReason::RateLimited,
        TruncationReason::Cancelled,
    ] {
        let name = reason.as_str();
        assert!(
            name.chars().all(|c| c.is_ascii_uppercase() || c == '_') && !name.is_empty(),
            "{name} is not a stable SCREAMING_SNAKE_CASE name"
        );
    }
}
