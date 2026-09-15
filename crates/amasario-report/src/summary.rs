//! The one-paragraph summary a caller prints when it is not printing a report.
//!
//! A summary is not a report and must not read like one. It states counts and the
//! epistemic split, and it never states a conclusion the sections do not already
//! contain: a summary that said "no dependencies found" without saying whether the
//! search was bounded would be exactly the overstatement the specification forbids.
//!
//! The disclaimers are not repeated here. A summary is read alongside the report it
//! summarises, and the report carries them; a line of disclaimer attached to every
//! short line of output is the kind of noise that stops being read at all.

use crate::model::Report;

/// Renders a one-paragraph summary.
#[must_use]
pub fn render(report: &Report) -> String {
    let sections = &report.sections;
    let mut line = format!(
        "{}: {} observed fact(s), {} inferred relationship(s), {} verification outcome(s), \
         {} confidence value(s), {} unanswered question(s), {} error(s)",
        report.target,
        sections.observed_facts.len(),
        sections.inferred_relationships.len(),
        sections.verification.len(),
        sections.confidence.len(),
        sections.unknown.len(),
        sections.errors.len()
    );

    if let Some(boundary) = &report.boundary {
        line.push_str(&format!(
            "; observed on {} at ledger {}",
            boundary.network.id, boundary.ledger
        ));
    }
    if !report.relationship_findings.is_empty() {
        line.push_str(&format!(
            "; {} relationship(s) explained",
            report.relationship_findings.len()
        ));
    }
    // The truncation note is the one thing a summary must never omit when it applies.
    // "Nothing else was found" and "the search stopped" produce the same counts and mean
    // opposite things.
    if report.truncated == Some(true) {
        line.push_str("; the analysis was bounded, so the counts are not a claim of completeness");
    }
    line.push('.');
    line
}

/// Renders the verification outcomes as a short list, for a caller that wants them
/// without the whole report.
#[must_use]
pub fn verification_lines(report: &Report) -> Vec<String> {
    report
        .sections
        .verification
        .iter()
        .map(|entry| match &entry.detail {
            Some(detail) => format!("{}: {} - {detail}", entry.subject, entry.status),
            None => format!("{}: {}", entry.subject, entry.status),
        })
        .collect()
}
