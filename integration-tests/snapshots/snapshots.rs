//! The snapshot layer, and the `amasario diff` command end to end.
//!
//! # Why the snapshots are read through the store
//!
//! `store::read` verifies the content digest on the way in, so a fixture that loaded
//! here is a fixture whose digest agrees with its own contents. That is not a formality:
//! the digest is computed over the canonical form with the declared volatile fields
//! excluded, so a snapshot that had been reformatted by hand, reordered by a tool, or
//! edited at all would fail. Reading through the store is therefore the strongest
//! available statement that the committed bytes are the bytes the engine wrote.
//!
//! # Why the diff is driven through the binary
//!
//! The library-level comparison is unit-tested in `amasario-snapshot`. What is not is
//! the path a user takes: `amasario diff --before <file> --after <file>`. That path has
//! to find the files, parse them, compare them, render the result and exit zero, and it
//! is the path the `Integration` workflow runs. So it is exercised here, against the
//! committed fixtures, with no network involved.

use amasario_integration_tests::cli;
use amasario_integration_tests::corpus::{Corpus, assert_committed};
use amasario_snapshot::{ChangeCategory, Snapshot, capture::snapshot_id, compare, store};

/// The two fixture file names, in one place.
const BEFORE: &str = "testnet-alpha-before.json";
const AFTER: &str = "testnet-alpha-after.json";

fn fixture(file: &str) -> Snapshot {
    let path = Corpus::path("snapshots", file);
    store::read(&path).unwrap_or_else(|error| panic!("{file} could not be read: {error}"))
}

/// Every snapshot fixture is committed, and what is committed is what the model produces.
#[test]
fn every_snapshot_fixture_matches_its_builder() {
    let (before, after) = amasario_integration_tests::documents::snapshots();
    assert_committed(
        "snapshots",
        BEFORE,
        &amasario_integration_tests::documents::rendered(&before),
    );
    assert_committed(
        "snapshots",
        AFTER,
        &amasario_integration_tests::documents::rendered(&after),
    );
}

/// Both fixtures pass every check the store applies on read.
#[test]
fn both_snapshots_are_valid_with_no_failures() {
    for file in [BEFORE, AFTER] {
        let snapshot = fixture(file);
        let failures = snapshot.failures();
        assert!(failures.is_empty(), "{file}: {failures:#?}");
        snapshot
            .validate()
            .unwrap_or_else(|error| panic!("{file}: {error}"));
    }
}

/// A snapshot's identifier is derived from its contract and its boundary.
///
/// Asserted because it is what makes a re-capture of an unchanged state *the same
/// snapshot* rather than a new one, which is the difference between a scheduled diff
/// that alerts on real change and one that alerts every night.
#[test]
fn a_snapshots_identifier_is_the_one_its_contract_and_boundary_derive() {
    for file in [BEFORE, AFTER] {
        let snapshot = fixture(file);
        assert_eq!(
            snapshot.id,
            snapshot_id(&snapshot.contract, &snapshot.boundary),
            "{file}: the identifier is not the derived one"
        );
        assert!(
            snapshot
                .id
                .starts_with(amasario_snapshot::SNAPSHOT_ID_PREFIX)
        );
    }
}

/// Both captures are of one contract at one boundary.
///
/// If they were not, the comparison below would be reporting a difference between two
/// contracts rather than between two states of one.
#[test]
fn both_captures_describe_one_contract_at_one_boundary() {
    let before = fixture(BEFORE);
    let after = fixture(AFTER);
    assert_eq!(
        before.contract, after.contract,
        "the pair must be about the same contract, or the diff compares two subjects"
    );
    assert_eq!(before.network, after.network);
    assert_eq!(before.boundary.ledger, after.boundary.ledger);
}

/// The pair differs in the ways the diff is supposed to report.
#[test]
fn the_pair_differs_in_more_than_one_category() {
    let before = fixture(BEFORE);
    let after = fixture(AFTER);
    let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("the pair compares");

    assert!(
        diff.comparable,
        "both captures are of the same contract and chain"
    );
    assert!(
        diff.validate().is_ok(),
        "the diff must satisfy its own rules: {:?}",
        diff.failures()
    );

    let categories: Vec<ChangeCategory> = diff.changes.iter().map(|entry| entry.category).collect();
    assert!(
        !categories.is_empty(),
        "the fixtures exist to produce a non-empty difference"
    );

    // The dependency surface changed, so the diff has to say so. This is the assertion
    // that fails if a future change made the comparison blind to the edge set.
    assert!(
        categories.contains(&ChangeCategory::RelationshipAdded),
        "an edge was added between the two captures; categories were {categories:?}"
    );

    // And the comparison reports more than one kind of change, which is what makes the
    // fixture worth having: a diff that could only see added edges would pass a weaker
    // test.
    let distinct: std::collections::BTreeSet<&str> = categories
        .iter()
        .map(|category| category.as_str())
        .collect();
    assert!(
        distinct.len() > 1,
        "the pair differs in more than one way; categories were {categories:?}"
    );
}

/// Every diff entry explains itself.
///
/// A diff is the artefact most likely to be consumed without review, so a reviewer needs
/// something to reject an entry on. Every entry carries a reason and a change type.
#[test]
fn every_diff_entry_states_a_reason_and_a_change_type() {
    let before = fixture(BEFORE);
    let after = fixture(AFTER);
    let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("the pair compares");
    for entry in &diff.changes {
        assert!(!entry.reason.is_empty(), "{} has no reason", entry.id);
        assert!(
            entry.reason.chars().count() >= 8,
            "{} has a reason too short to reject anything: {}",
            entry.id,
            entry.reason
        );
        let _ = entry.change_type;
        assert!(
            entry.before_value.is_some() || entry.after_value.is_some(),
            "{} reports a difference with neither side recorded",
            entry.id
        );
    }
}

/// Comparing a capture with itself produces no differences.
///
/// The negative case, and the one that keeps a scheduled diff usable: a comparison that
/// reported the capture time as a change would fire every night and be ignored within a
/// week. The volatile fields are excluded from the digest for exactly this reason.
#[test]
fn a_snapshot_compared_with_itself_reports_nothing() {
    let snapshot = fixture(BEFORE);
    let diff = compare(&snapshot, &snapshot, "2026-01-02T00:00:00Z").expect("comparable");
    assert!(diff.comparable);
    assert!(
        diff.changes.is_empty(),
        "a capture compared with itself must produce no changes, got {:?}",
        diff.changes
            .iter()
            .map(|entry| entry.reason.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(diff.summary.as_ref().map(|summary| summary.total), Some(0));
}

/// The comparison runs in canonical mode, so two runs produce the same document.
#[test]
fn the_comparison_is_deterministic() {
    let before = fixture(BEFORE);
    let after = fixture(AFTER);
    let first = compare(&before, &after, "2026-01-02T00:00:00Z").expect("comparable");
    let second = compare(&before, &after, "2026-01-02T00:00:00Z").expect("comparable");
    assert_eq!(first.changes, second.changes);
    assert_eq!(
        serde_json::to_string(&first).expect("serialises"),
        serde_json::to_string(&second).expect("serialises")
    );
}

/// Two snapshots from different chains cannot be compared, and say so.
///
/// An incomparable pair is a result rather than an error, and the reason is recorded:
/// a caller that received an error could not tell "these cannot be compared" from "the
/// comparison failed", and only one of those is worth retrying.
#[test]
fn a_pair_from_different_chains_is_incomparable_with_a_reason() {
    let mut after = fixture(AFTER);
    after.network = amasario_core::Network::new(
        "mainnet",
        amasario_core::NetworkType::Mainnet,
        "Public Global Stellar Network ; September 2015",
    )
    .expect("a network");
    after.boundary.network = after.network.clone();
    after.id = snapshot_id(&after.contract, &after.boundary);

    let before = fixture(BEFORE);
    let diff = compare(&before, &after, "2026-01-02T00:00:00Z").expect("the comparison runs");
    assert!(!diff.comparable);
    assert!(
        diff.incomparable_reason.is_some(),
        "an incomparable pair must say why"
    );
    assert!(
        diff.changes.is_empty(),
        "an incomparable pair reports no changes, because it has nothing to compare"
    );
}

/// The fixture names are the ones the integration workflow looks for.
///
/// The workflow globs `fixtures/snapshots/*before*.json` and `*after*.json` to find a
/// pair to feed the binary. A rename that broke the glob would silently stop the
/// workflow exercising `diff`, so the naming is asserted rather than left to chance.
#[test]
fn the_fixture_names_match_what_the_workflow_globs_for() {
    let files: Vec<String> = Corpus::files_in("snapshots")
        .into_iter()
        .map(|path| {
            path.file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        files.iter().any(|name| name.contains("before")),
        "no snapshot fixture name contains 'before': {files:?}"
    );
    assert!(
        files.iter().any(|name| name.contains("after")),
        "no snapshot fixture name contains 'after': {files:?}"
    );
    assert_eq!(
        files.len(),
        2,
        "the corpus holds exactly one pair: {files:?}"
    );
}

/// `amasario diff` runs over the committed fixtures and exits zero.
#[test]
fn the_diff_command_runs_over_the_committed_pair() {
    if !cli::is_built() {
        panic!(
            "the amasario binary has not been built, so the end-to-end path cannot be \
             checked. Build it with `cargo build -p amasario-cli`, which \
             `cargo test --workspace` does for you."
        );
    }

    let before = Corpus::path("snapshots", BEFORE);
    let after = Corpus::path("snapshots", AFTER);
    let output = cli::run(&[
        "diff",
        "--before",
        &before.to_string_lossy(),
        "--after",
        &after.to_string_lossy(),
        "--format",
        "json",
    ]);

    assert!(
        output.status.success(),
        "diff exited {:?}: {}",
        output.status.code(),
        cli::stderr(&output)
    );

    let rendered = cli::stdout(&output);
    let document: serde_json::Value = serde_json::from_str(&rendered)
        .unwrap_or_else(|error| panic!("diff did not write a document: {error}\n{rendered}"));
    assert_eq!(
        document["comparable"],
        serde_json::Value::Bool(true),
        "the committed pair is comparable"
    );
    assert!(
        document["changes"]
            .as_array()
            .is_some_and(|changes| !changes.is_empty()),
        "the committed pair differs"
    );
}

/// `amasario diff` refuses a file that is not a snapshot, with a non-zero status.
///
/// The negative case: a tool that exited zero on unreadable input would be one a
/// workflow could not act on.
#[test]
fn the_diff_command_refuses_a_file_that_is_not_a_snapshot() {
    if !cli::is_built() {
        panic!("the amasario binary has not been built; see the test above");
    }

    let directory = tempfile::tempdir().expect("a temporary directory");
    let not_a_snapshot = directory.path().join("not-a-snapshot.json");
    std::fs::write(&not_a_snapshot, b"{\"this\": \"is not a snapshot\"}")
        .expect("the file is writable");
    let before = Corpus::path("snapshots", BEFORE);

    let output = cli::run(&[
        "diff",
        "--before",
        &before.to_string_lossy(),
        "--after",
        &not_a_snapshot.to_string_lossy(),
        "--format",
        "json",
    ]);

    assert!(
        !output.status.success(),
        "diff must not exit zero on a file it could not read"
    );
    assert!(
        !cli::stderr(&output).is_empty(),
        "a refusal must say what was wrong"
    );
}
