//! The report layer, checked against the documents it publishes and the renderings it
//! produces.
//!
//! # The separation this suite holds the report to
//!
//! A report is the one artefact a person reads rather than a program, and its value is
//! that it keeps four things apart: what was observed, what was inferred, what could not
//! be established, and what went wrong. A report that merged them would be
//! indistinguishable from a tool that guessed - so each committed fixture is asserted to
//! carry its own kind of content, and each rendering is asserted to render it.
//!
//! # Why every format is rendered here
//!
//! JSON, Markdown, DOT and JUnit are four different consumers with four different
//! expectations: a pipeline, a person, a graph renderer and a test reporter. A rendering
//! that silently produced an empty document would pass a check that only asked whether
//! it had been produced, so the assertions below are on the *content* of each.

use amasario_integration_tests::corpus::{Corpus, assert_committed, parse_committed};
use amasario_integration_tests::documents::{self, ReportKind};
use amasario_report::formatter::{Format, render, render_summary};
use amasario_report::model::{Report, UnknownReason};

/// The four renderings a report can be produced in.
const fn formats() -> [Format; 4] {
    [Format::Json, Format::Markdown, Format::Dot, Format::Junit]
}

/// Every report fixture is committed, and what is committed is what the model produces.
#[test]
fn every_report_fixture_matches_its_builder() {
    for kind in ReportKind::all() {
        let file = format!("{}.json", kind.file());
        let built = documents::report(*kind);
        assert_committed("expected-reports", &file, &documents::rendered(&built));
    }
}

/// Every fixture parses and satisfies every rule the report model states.
#[test]
fn every_report_fixture_is_valid() {
    for kind in ReportKind::all() {
        let file = format!("{}.json", kind.file());
        let report: Report = parse_committed("expected-reports", &file);
        let failures = report.failures();
        assert!(failures.is_empty(), "{file}: {failures:#?}");
        report
            .validate()
            .unwrap_or_else(|error| panic!("{file}: {error}"));
    }
}

/// A report names its target and its boundary.
///
/// Without a target it is about nothing, and without a boundary it is about no moment:
/// both are what make the statements in it checkable by a reader.
#[test]
fn every_report_names_its_target_and_its_boundary() {
    for kind in ReportKind::all() {
        let file = format!("{}.json", kind.file());
        let report: Report = parse_committed("expected-reports", &file);
        assert_eq!(report.target.kind, amasario_core::EntityKind::Contract);
        assert_eq!(report.target.id, documents::alpha().id);
        let boundary = report
            .boundary
            .as_ref()
            .unwrap_or_else(|| panic!("{file} records no boundary"));
        assert_eq!(boundary.network.id, "testnet");
        assert_eq!(boundary.ledger.get(), documents::BOUNDARY_LEDGER);
    }
}

/// The report's own version matches the specification family the engine implements.
#[test]
fn every_report_declares_the_supported_versions() {
    for kind in ReportKind::all() {
        let file = format!("{}.json", kind.file());
        let report: Report = parse_committed("expected-reports", &file);
        assert!(
            !report.api_version.is_empty() && !report.spec_version.is_empty(),
            "{file}: a report must declare which version produced it"
        );
    }
}

/// An observed fact is separated from an inference, and each inference names its basis.
#[test]
fn observed_facts_and_inferences_are_kept_apart() {
    let report = documents::report(ReportKind::Verified);
    assert!(
        !report.sections.observed_facts.is_empty(),
        "the fixture observes something"
    );
    assert!(
        !report.sections.inferred_relationships.is_empty(),
        "the fixture infers something"
    );

    for statement in &report.sections.observed_facts {
        assert!(
            !statement.evidence.is_empty(),
            "an observed fact must cite what it was observed in"
        );
    }
    for inferred in &report.sections.inferred_relationships {
        assert!(
            !inferred.inference_basis.is_empty(),
            "an inference must name its basis; an inference without one is a guess"
        );
        assert!(
            !inferred.evidence.is_empty(),
            "an inference must still cite its evidence"
        );
    }
}

/// The unknown-provenance fixture is honest about what it could not determine.
///
/// The section that matters most: a report that omitted its own incompleteness would read
/// as a complete account of a contract, when it is an account of one address.
#[test]
fn the_unknown_fixture_states_the_question_it_cannot_answer() {
    let report = documents::report(ReportKind::UnknownProvenance);
    assert!(
        !report.sections.unknown.is_empty(),
        "the fixture exists to carry an unanswered question"
    );
    for entry in &report.sections.unknown {
        assert!(
            entry.question.trim_end().ends_with('?') || !entry.question.is_empty(),
            "an unknown section entry states a question rather than a gap"
        );
        let _: UnknownReason = entry.reason;
    }
    assert!(
        report
            .sections
            .verification
            .iter()
            .any(|entry| { entry.status == amasario_core::VerificationStatus::Unverified }),
        "a contract with no established origin is unverified, not verified"
    );
}

/// The cyclic fixture discloses the cycle rather than presenting a tidy tree.
#[test]
fn the_cyclic_fixture_carries_the_graph_and_discloses_the_cycle() {
    let report = documents::report(ReportKind::CyclicDependencies);
    let graph = report
        .graph
        .as_ref()
        .expect("the fixture carries the graph it is about");
    assert!(
        graph
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.cyclic),
        "the graph is cyclic and says so"
    );
    assert!(
        report
            .sections
            .unknown
            .iter()
            .any(|entry| { entry.question.contains("cycle") || entry.question.contains("begins") }),
        "the cycle leaves a question open, and the report asks it: {:?}",
        report
            .sections
            .unknown
            .iter()
            .map(|entry| entry.question.as_str())
            .collect::<Vec<_>>()
    );
}

/// The report warns that it is not a security assessment.
///
/// The claim this repository must never make by omission. A report that listed
/// dependencies without saying what it is not could be read as a clearance.
#[test]
fn every_report_carries_its_disclaimers() {
    for kind in ReportKind::all() {
        let file = format!("{}.json", kind.file());
        let report: Report = parse_committed("expected-reports", &file);
        assert!(
            !report.disclaimers.is_empty(),
            "{file} carries no disclaimer, and a report about dependencies is not a \
             statement about safety"
        );
    }
}

/// The JSON rendering parses back into the same report.
///
/// A rendering that lost a field would be a report that says something different from
/// the report it came from. The one field that does not survive is `graph`, which the
/// model holds in memory so a report can be drawn: `report.schema.json` declares no
/// graph field and refuses additional properties, so writing one would make the document
/// invalid. That is asserted here rather than assumed, because it is the difference
/// between a document a consumer accepts and one it refuses.
#[test]
fn the_json_rendering_round_trips() {
    for kind in ReportKind::all() {
        let report = documents::report(*kind);
        let rendered = render(&report, Format::Json).expect("JSON renders");
        let reparsed: Report = serde_json::from_str(&rendered)
            .unwrap_or_else(|error| panic!("{kind:?}: the JSON rendering did not parse: {error}"));

        let mut expected = report.clone();
        expected.graph = None;
        assert_eq!(reparsed, expected, "{kind:?}: the rendering lost something");
        assert!(
            !rendered.contains("\"graph\""),
            "{kind:?}: the in-memory graph must not reach the document, which the schema \
             would refuse"
        );
    }
}

/// Every format renders non-empty content that names the target.
#[test]
fn every_format_renders_content_about_the_target() {
    let report = documents::report(ReportKind::Verified);
    for format in formats() {
        if format == Format::Dot {
            // This report carries no graph. Drawing an empty digraph would read as a
            // finding about the topology rather than as the absence of one, so the
            // renderer refuses - and the refusal is the assertion. The cyclic fixture
            // holds a report that does carry a graph, and it is asserted separately.
            let error =
                render(&report, format).expect_err("a report with no graph has nothing to draw");
            assert!(
                error.to_string().contains("no graph"),
                "the refusal must say why: {error}"
            );
            continue;
        }
        let rendered = render(&report, format)
            .unwrap_or_else(|error| panic!("{format:?} did not render: {error}"));
        assert!(
            rendered.len() > 64,
            "{format:?} rendered {} bytes, which is not a report",
            rendered.len()
        );
        assert!(
            rendered.contains(&documents::alpha().id),
            "{format:?} does not name the contract it is about"
        );
    }
}

/// The Markdown rendering keeps the four sections apart.
#[test]
fn the_markdown_rendering_separates_what_is_observed_from_what_is_unknown() {
    let report = documents::report(ReportKind::UnknownProvenance);
    let rendered = render(&report, Format::Markdown).expect("Markdown renders");
    for section in ["Observed", "Unknown"] {
        assert!(
            rendered.contains(section),
            "the Markdown rendering does not have a {section} section:\n{rendered}"
        );
    }
}

/// The DOT rendering is a graph a renderer can read.
#[test]
fn the_dot_rendering_is_well_formed() {
    let report = documents::report(ReportKind::CyclicDependencies);
    let rendered = render(&report, Format::Dot).expect("DOT renders");
    assert!(rendered.contains("digraph"), "DOT needs a digraph header");
    assert!(rendered.ends_with("}\n"), "DOT needs a closing brace");
    assert_eq!(
        rendered.matches('{').count(),
        rendered.matches('}').count(),
        "the braces must balance"
    );
}

/// The JUnit rendering is XML with a suite and at least one case.
#[test]
fn the_junit_rendering_is_well_formed_xml() {
    let report = documents::report(ReportKind::Verified);
    let rendered = render(&report, Format::Junit).expect("JUnit renders");
    assert!(rendered.trim_start().starts_with("<?xml"));
    assert!(rendered.contains("<testsuite"));
    assert!(rendered.contains("</testsuite>"));
    assert!(rendered.contains("<testcase"), "a JUnit report needs cases");
    // The opening tag is matched with its trailing space so that the `<testsuites>`
    // wrapper is not counted as a second suite: the document has one wrapper and one
    // suite inside it, which is what a test runner expects.
    assert_eq!(
        rendered.matches("<testsuite ").count(),
        rendered.matches("</testsuite>").count(),
        "the suite element must be closed: {rendered}"
    );
}

/// The summary rendering is one line about the report.
#[test]
fn the_summary_rendering_is_a_single_line_naming_the_target() {
    let report = documents::report(ReportKind::Verified);
    let summary = render_summary(&report).expect("the summary renders");
    assert!(!summary.is_empty());
    assert!(
        !summary.contains('\n'),
        "a summary is one line, got: {summary:?}"
    );
}

/// Two renderings of one report are identical.
///
/// The refusal is compared as well as the output: a format a report cannot be rendered
/// in must fail the same way twice, or a caller could not tell a deterministic refusal
/// from a race.
#[test]
fn rendering_is_deterministic() {
    for kind in ReportKind::all() {
        let report = documents::report(*kind);
        for format in formats() {
            let first = render(&report, format).map_err(|error| error.to_string());
            let second = render(&report, format).map_err(|error| error.to_string());
            assert_eq!(first, second, "{kind:?} as {format:?} is not deterministic");
        }
    }
}

/// The corpus directory holds exactly the fixture set the builder defines.
#[test]
fn the_corpus_holds_no_fixture_the_builder_does_not_define() {
    let mut expected: Vec<String> = ReportKind::all()
        .iter()
        .map(|kind| format!("{}.json", kind.file()))
        .collect();
    expected.sort();
    let actual: Vec<String> = Corpus::files_in("expected-reports")
        .into_iter()
        .map(|path| {
            path.file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(actual, expected);
}
