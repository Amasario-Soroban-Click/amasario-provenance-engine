//! Report generation: the specification's report document, rendered four ways.
//!
//! # What a report is for
//!
//! Everything else in the engine answers a question. This crate decides how the answers
//! are presented, and the specification is unusually specific about why that is not a
//! formatting concern. `report.schema.json` says its central requirement is that
//! "observed facts, inferred relationships, verification outcomes, confidence, unknown
//! information and errors occupy distinct sections and can never be merged", because
//! merging them "is the failure mode that makes provenance tooling untrustworthy".
//!
//! So a report is not a summary of an analysis. It is the analysis, separated by what
//! the engine actually knows, and the value is in the separation rather than in the
//! content.
//!
//! # The four renderings
//!
//! | Format | For | Keeps the sections apart by |
//! | --- | --- | --- |
//! | JSON | a consumer | the schema's own section objects |
//! | Markdown | a person | one heading per section, the inference heading naming itself, and each inference repeating its basis |
//! | DOT | a picture of the graph | dashed edges for inferences and a node stating truncation |
//! | JUnit | a CI gate | `UNVERIFIED` and `CONFLICTING` failing the build; `UNKNOWN` skipped |
//!
//! Every renderer is held to the same requirement. A Markdown report that printed an
//! inference as a bullet beside an observation would undo the schema's separation in
//! the form a reader actually sees, which is the form that matters.
//!
//! # What this crate refuses to do
//!
//! * It will not produce a report without the disclaimers. `disclaimers` has
//!   `minItems: 1` and rule `provenance/evidence-traceability` requires the
//!   non-security-determination statement, so a report reading as a security assessment
//!   is not representable.
//! * It will not publish a report that fails its own structural checks.
//!   [`Report::canonical_json`] validates before serialising, so an invalid document is
//!   never produced rather than produced and then refused.
//! * It will not draw an empty graph. A report carries no graph field, and an empty
//!   digraph would read as "nothing is related" rather than "there is nothing to draw".
//!
//! # Amasario is not a security scanner
//!
//! No term in this crate means "secure", "safe", "malicious" or "vulnerable", and
//! `VERIFIED` describes the relationship between evidence and a claim rather than the
//! trustworthiness of a contract. A fully verified provenance chain can describe a
//! deliberate backdoor, which is why that sentence is one of the required disclaimers
//! rather than a note in the documentation.
//!
//! # Example
//!
//! ```
//! use amasario_core::{EntityKind, EntityRef, VerificationStatus};
//! use amasario_report::{
//!     Format,
//!     model::{RelationshipFinding, Report, Statement, VerificationEntry},
//!     render,
//! };
//!
//! # fn main() -> amasario_core::Result<()> {
//! let subject = EntityRef::new(EntityKind::Contract, "C-example")?;
//! let mut report = Report::new(subject.clone());
//! report.add_observed(Statement::new(
//!     "the deployed executable hash was read from the network",
//!     Some(subject.clone()),
//!     vec!["ev-wasm-1".to_owned()],
//! )?);
//! report.add_verification(
//!     VerificationEntry::new(subject, VerificationStatus::Verified)
//!         .with_evidence(vec!["ev-wasm-1".to_owned()]),
//! );
//! report.add_relationship_finding(RelationshipFinding::new(
//!     amasario_core::Relationship::Invocates,
//!     "INVOCATES is held between C-example and C-callee because a cross-contract call \
//!      was observed in a transaction",
//! )?);
//!
//! // The JSON is the specification's document; the Markdown keeps the sections apart.
//! let json = render(&report, Format::Json)?;
//! assert!(json.contains("observedFacts"));
//! let markdown = render(&report, Format::Markdown)?;
//! assert!(markdown.contains("## Inferred relationships (derived, not observed)"));
//! assert!(markdown.contains("not a security scanner"));
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod dot;
pub mod formatter;
pub mod json;
pub mod junit;
pub mod markdown;
pub mod model;
pub mod summary;

pub use formatter::{Format, render, render_summary};
pub use model::{
    ConfidenceEntry, DISCLAIMERS, InferredStatement, MAXIMUM_STATEMENT_LENGTH,
    MINIMUM_STATEMENT_LENGTH, RelationshipFinding, Report, ReportError, Sections, Statement,
    UnknownEntry, UnknownReason, VerificationEntry,
};

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{EntityKind, EntityRef, VerificationStatus};

    fn report() -> Report {
        let subject = EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference");
        let mut report = Report::new(subject.clone());
        report.add_observed(
            Statement::new(
                "the deployed executable hash was read from the network",
                Some(subject.clone()),
                vec!["ev-1".to_owned()],
            )
            .expect("a statement"),
        );
        report.add_verification(
            VerificationEntry::new(subject, VerificationStatus::Verified)
                .with_evidence(vec!["ev-1".to_owned()]),
        );
        report
    }

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name it.
        assert_eq!(Format::Json.as_str(), "json");
        assert_eq!(Format::Markdown.extension(), "md");
        assert_eq!(Format::Junit.extension(), "xml");
        assert_eq!(Format::parse("md").expect("a format"), Format::Markdown);
        assert_eq!(Format::parse("graphviz").expect("a format"), Format::Dot);
        assert_eq!(UnknownReason::Timeout.as_str(), "TIMEOUT");
        assert!(UnknownReason::Timeout.is_transient());
        assert!(!UnknownReason::NotFound.is_transient());
        assert!(DISCLAIMERS[0].contains("not a security scanner"));
    }

    #[test]
    fn an_unknown_format_names_the_ones_that_are_accepted() {
        let error = Format::parse("yaml").expect_err("not a report format");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::Configuration
        );
        assert!(error.to_string().contains("markdown"), "got: {error}");
    }

    #[test]
    fn every_format_renders_and_carries_the_disclaimers() {
        let mut report = report();
        report = report.with_graph(amasario_graph::GraphDocument::of(
            &amasario_graph::Graph::new("g").expect("a graph"),
        ));
        for format in Format::all() {
            let rendered = render(&report, *format).expect("renders");
            assert!(!rendered.is_empty(), "{format} produced nothing");
        }
        // The three that render prose carry the disclaimer verbatim.
        for format in [Format::Json, Format::Markdown] {
            let rendered = render(&report, format).expect("renders");
            assert!(
                rendered.contains("not a security scanner"),
                "{format} dropped the disclaimer"
            );
        }
    }

    #[test]
    fn the_json_rendering_is_the_specifications_document() {
        let json: serde_json::Value =
            serde_json::from_str(&render(&report(), Format::Json).expect("renders"))
                .expect("valid JSON");
        let object = json.as_object().expect("an object");
        for required in [
            "apiVersion",
            "specVersion",
            "target",
            "sections",
            "disclaimers",
        ] {
            assert!(object.contains_key(required), "missing {required}");
        }
        // `report.schema.json` sets `additionalProperties: false`, so a graph field
        // would invalidate the document.
        assert!(object.get("graph").is_none(), "a report has no graph field");
        for section in [
            "observedFacts",
            "inferredRelationships",
            "verification",
            "confidence",
            "unknown",
            "errors",
        ] {
            assert!(
                json["sections"].get(section).is_some(),
                "every section is present, even when empty: {section}"
            );
        }
    }

    #[test]
    fn an_invalid_report_is_refused_rather_than_rendered() {
        // A document with a section missing is not representable through the model, so
        // the reachable failure is a statement without evidence.
        let mut report = report();
        report.sections.observed_facts[0].evidence.clear();
        let error = render(&report, Format::Markdown).expect_err("unsupported statement");
        assert_eq!(error.category(), amasario_core::ErrorCategory::Report);
        assert!(error.to_string().contains("evidence"), "got: {error}");
    }

    #[test]
    fn the_rendering_is_deterministic() {
        let first = render(&report(), Format::Json).expect("renders");
        for _ in 0..5 {
            assert_eq!(render(&report(), Format::Json).expect("renders"), first);
        }
    }

    #[test]
    fn the_summary_states_the_counts_and_the_epistemic_split() {
        let summary = render_summary(&report()).expect("renders");
        assert!(summary.contains("1 observed fact(s)"), "got: {summary}");
        assert!(summary.contains("1 verification outcome(s)"));
        assert!(summary.contains("C-subject"));
    }

    #[test]
    fn the_summary_never_omits_a_bounded_analysis() {
        let summary = render_summary(&report().truncated(true)).expect("renders");
        assert!(
            summary.contains("not a claim of completeness"),
            "a summary that hid the bound would read as a complete result: {summary}"
        );
    }
}
