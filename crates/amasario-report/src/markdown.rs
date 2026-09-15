//! The Markdown renderer.
//!
//! # Why the headings carry the epistemic weight
//!
//! `report.schema.json` separates observations from inferences by putting them in
//! different arrays. A rendering that printed both as bullets under one heading would
//! undo that in the form a person actually reads, which is the whole failure the
//! separation exists to prevent. So each section gets its own heading, the inference
//! section says in the heading that these are inferences, and every inference repeats
//! its basis on its own line rather than relying on the reader having noticed which
//! heading they are under.
//!
//! The disclaimers are rendered last, at the top level rather than inside a section,
//! because they qualify the whole document rather than any part of it.

use amasario_core::Result;

use crate::model::Report;

/// Renders a report as Markdown.
///
/// # Errors
///
/// Returns a report error when the report is structurally invalid.
pub fn render(report: &Report) -> Result<String> {
    report.validate()?;
    let mut out = String::new();

    out.push_str(&format!("# Amasario report: {}\n\n", report.target));
    if let Some(boundary) = &report.boundary {
        out.push_str(&format!(
            "Observed on **{}** at ledger **{}** on {}.\n\n",
            boundary.network.id, boundary.ledger, boundary.observed_at
        ));
    }
    if report.truncated == Some(true) {
        // Stated before any finding, because a reader who has already concluded
        // "nothing else exists" reads the rest of the document differently.
        out.push_str(
            "> **The analysis was bounded and did not run to exhaustion.** An absent \
             relationship below is not a statement that no relationship exists.\n\n",
        );
    }

    render_observed(report, &mut out);
    render_inferred(report, &mut out);
    render_verification(report, &mut out);
    render_confidence(report, &mut out);
    render_unknown(report, &mut out);
    render_errors(report, &mut out);
    render_relationships(report, &mut out);
    render_disclaimers(report, &mut out);

    Ok(out)
}

/// Renders the observed facts, which are the only statements with no basis to state.
fn render_observed(report: &Report, out: &mut String) {
    out.push_str("## Observed facts\n\n");
    if report.sections.observed_facts.is_empty() {
        out.push_str("_None._\n\n");
        return;
    }
    for statement in &report.sections.observed_facts {
        out.push_str(&format!("- {}\n", statement.statement));
        if let Some(entity) = &statement.entity {
            out.push_str(&format!("  - entity: `{entity}`\n"));
        }
        out.push_str(&format!("  - evidence: {}\n", inline(&statement.evidence)));
        if let Some(confidence) = &statement.confidence {
            out.push_str(&format!("  - confidence: {}\n", confidence.level));
        }
    }
    out.push('\n');
}

/// Renders the inferences, each stating its basis on its own line.
fn render_inferred(report: &Report, out: &mut String) {
    out.push_str("## Inferred relationships (derived, not observed)\n\n");
    if report.sections.inferred_relationships.is_empty() {
        out.push_str("_None._\n\n");
        return;
    }
    for statement in &report.sections.inferred_relationships {
        out.push_str(&format!("- {}\n", statement.statement));
        if let Some(entity) = &statement.entity {
            out.push_str(&format!("  - entity: `{entity}`\n"));
        }
        // On the entry itself, not only in the heading: a reader who is skimming must
        // not have to have read the heading to know this is an inference.
        out.push_str(&format!(
            "  - **inference basis:** {}\n",
            statement.inference_basis
        ));
        out.push_str(&format!("  - evidence: {}\n", inline(&statement.evidence)));
        if let Some(confidence) = &statement.confidence {
            out.push_str(&format!("  - confidence: {}\n", confidence.level));
        }
    }
    out.push('\n');
}

/// Renders the verification outcomes.
fn render_verification(report: &Report, out: &mut String) {
    out.push_str("## Verification\n\n");
    if report.sections.verification.is_empty() {
        out.push_str("_Nothing was checked._\n\n");
        return;
    }
    out.push_str("| Subject | Status | Detail |\n| --- | --- | --- |\n");
    for entry in &report.sections.verification {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            entry.subject,
            entry.status,
            entry.detail.as_deref().unwrap_or("")
        ));
    }
    out.push('\n');
}

/// Renders the confidence values.
fn render_confidence(report: &Report, out: &mut String) {
    out.push_str("## Confidence\n\n");
    if report.sections.confidence.is_empty() {
        out.push_str("_None assigned._\n\n");
        return;
    }
    for entry in &report.sections.confidence {
        out.push_str(&format!(
            "- `{}`: {} (evidence: {})\n",
            entry.subject,
            entry.confidence.level,
            inline(&entry.confidence.evidence)
        ));
        if let Some(rationale) = &entry.confidence.rationale {
            out.push_str(&format!("  - rationale: {rationale}\n"));
        }
        if !entry.confidence.contradicting_evidence.is_empty() {
            out.push_str(&format!(
                "  - contradicting evidence: {}\n",
                inline(&entry.confidence.contradicting_evidence)
            ));
        }
    }
    out.push('\n');
}

/// Renders the unanswered questions.
fn render_unknown(report: &Report, out: &mut String) {
    out.push_str("## Unknown\n\n");
    if report.sections.unknown.is_empty() {
        out.push_str("_No question went unanswered._\n\n");
        return;
    }
    for entry in &report.sections.unknown {
        out.push_str(&format!("- {} ({})\n", entry.question, entry.reason));
        if let Some(detail) = &entry.detail {
            out.push_str(&format!("  - {detail}\n"));
        }
    }
    out.push('\n');
}

/// Renders the failures.
fn render_errors(report: &Report, out: &mut String) {
    out.push_str("## Errors\n\n");
    if report.sections.errors.is_empty() {
        out.push_str("_None._\n\n");
        return;
    }
    for error in &report.sections.errors {
        out.push_str(&format!(
            "- `{}` ({}): {}\n",
            error.code, error.category, error.message
        ));
        match error.retryable {
            Some(true) => out.push_str("  - retryable: a later attempt could succeed\n"),
            Some(false) => out.push_str("  - not retryable: a later attempt would repeat this\n"),
            None => out.push_str("  - retryable: unknown\n"),
        }
    }
    out.push('\n');
}

/// Renders why each important relationship exists.
fn render_relationships(report: &Report, out: &mut String) {
    if report.relationship_findings.is_empty() {
        return;
    }
    out.push_str("## Why these relationships exist\n\n");
    for finding in &report.relationship_findings {
        out.push_str(&format!(
            "- **{}**: {}\n",
            finding.relationship, finding.explanation
        ));
        if let Some(edge) = &finding.edge_id {
            out.push_str(&format!("  - edge: `{edge}`\n"));
        }
        if finding.observationally_supported == Some(false) {
            out.push_str("  - this relationship was inferred, not observed\n");
        }
    }
    out.push('\n');
}

/// Renders the disclaimers.
fn render_disclaimers(report: &Report, out: &mut String) {
    out.push_str("---\n\n### Disclaimers\n\n");
    for disclaimer in &report.disclaimers {
        out.push_str(&format!("> {disclaimer}\n\n"));
    }
}

/// Renders a list of identifiers on one line.
fn inline(values: &[String]) -> String {
    if values.is_empty() {
        return "_none_".to_owned();
    }
    values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<String>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Statement, UnknownEntry, UnknownReason};
    use amasario_core::{EntityKind, EntityRef};

    fn report() -> Report {
        let mut report =
            Report::new(EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference"));
        report.add_observed(
            Statement::new(
                "the executable hash was read from the network",
                None,
                vec!["ev-1".to_owned()],
            )
            .expect("a statement"),
        );
        report.add_unknown(
            UnknownEntry::new(
                "which revision produced the deployed executable?",
                UnknownReason::EvidenceUnavailable,
            )
            .expect("a question"),
        );
        report
    }

    #[test]
    fn the_sections_keep_their_own_headings() {
        let rendered = render(&report()).expect("renders");
        assert!(rendered.contains("## Observed facts"));
        assert!(
            rendered.contains("## Inferred relationships (derived, not observed)"),
            "the inference heading says what it is"
        );
        assert!(rendered.contains("## Unknown"));
        assert!(rendered.contains("## Errors"));
    }

    #[test]
    fn an_empty_section_is_stated_rather_than_omitted() {
        // "An absent section and an empty section mean different things, and only one of
        // them is honest."
        let rendered = render(&report()).expect("renders");
        assert!(rendered.contains("_Nothing was checked._"));
        assert!(rendered.contains("_None._"));
    }

    #[test]
    fn the_disclaimers_are_always_rendered() {
        let rendered = render(&report()).expect("renders");
        assert!(rendered.contains("### Disclaimers"));
        assert!(rendered.contains("not a security scanner"));
        assert!(rendered.contains("can describe a deliberate backdoor"));
    }

    #[test]
    fn every_inference_states_its_basis_on_its_own_entry() {
        use crate::model::InferredStatement;
        let mut report = report();
        report.add_inferred(
            InferredStatement::new(
                "the contract depends on the package of the same name",
                None,
                vec!["ev-2".to_owned()],
                "a plain name match, which the specification says can never establish a dependency",
            )
            .expect("a statement"),
        );
        let rendered = render(&report).expect("renders");
        assert!(
            rendered.contains("**inference basis:**"),
            "an inference carries its basis on the entry, not only in the heading"
        );
    }

    #[test]
    fn a_truncated_analysis_says_so_before_the_first_finding() {
        let report = report().truncated(true);
        let rendered = render(&report).expect("renders");
        let warning = rendered
            .find("did not run to exhaustion")
            .expect("the warning");
        let first_section = rendered.find("## Observed facts").expect("the section");
        assert!(warning < first_section, "the warning precedes the findings");
    }
}
