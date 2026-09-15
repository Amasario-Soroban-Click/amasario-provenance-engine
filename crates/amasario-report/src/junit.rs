//! The JUnit XML renderer, so that a CI job can gate on a provenance check.
//!
//! # The mapping, and why it is not a score
//!
//! A JUnit document has exactly four things a case can be: passed, failed, errored or
//! skipped. The specification has five verification statuses. The mapping therefore
//! states what a build should do rather than compressing the statuses into a number:
//!
//! | Verification status | JUnit | Why |
//! | --- | --- | --- |
//! | `VERIFIED` | passed | the evidence is consistent with the claim |
//! | `PARTIALLY_VERIFIED` | passed, with the gap in the output | part of the claim holds; a build does not need to stop |
//! | `CONFLICTING` | failed | the claim is contradicted, which is the one outcome that must stop a build |
//! | `UNVERIFIED` | failed | nothing supports the claim, and treating that as a pass is how an unverified artifact ships |
//! | `UNKNOWN` | skipped | the question could not be asked, which is neither a pass nor a failure |
//!
//! `CONFLICTING` failing the build is the important one. It is the status that exists
//! because a claimed source revision that rebuilds to a different digest must not be
//! reported as verified, and the same reasoning applies to a CI gate: a conflict is the
//! condition a maintainer most needs to be told about and least likely to be told about
//! by a job that only counts.
//!
//! # Errors are separate from failures
//!
//! JUnit distinguishes a test that failed from a run that could not complete. The
//! report's `errors` section maps onto `<error>`, not `<failure>`, because "the network
//! did not answer" and "the answer was that the claim is contradicted" are different
//! findings - the distinction the whole error model exists to preserve.

use amasario_core::{Result, VerificationStatus};

use crate::model::{Report, ReportError};

/// Renders a report as a JUnit XML document.
///
/// # Errors
///
/// Returns a report error when the report is structurally invalid.
pub fn render(report: &Report) -> Result<String> {
    report.validate()?;

    let cases = report.sections.verification.len();
    let failures = report
        .sections
        .verification
        .iter()
        .filter(|entry| is_failure(entry.status))
        .count();
    let skipped = report
        .sections
        .verification
        .iter()
        .filter(|entry| entry.status == VerificationStatus::Unknown)
        .count();
    let errors = report.sections.errors.len();

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites name=\"amasario\" tests=\"{cases}\" failures=\"{failures}\" \
         errors=\"{errors}\" skipped=\"{skipped}\">\n"
    ));
    out.push_str(&format!(
        "<testsuite name=\"amasario provenance: {}\" tests=\"{cases}\" \
         failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\">\n",
        escape(&report.target.to_string())
    ));

    for entry in &report.sections.verification {
        out.push_str(&format!(
            "  <testcase name=\"{}\" classname=\"{}\">\n",
            escape(&entry.subject.to_string()),
            escape(entry.status.as_str())
        ));
        // The gap is always in the output, whatever the status, so that a passing case
        // still says what it did not establish.
        let mut detail = entry.detail.clone().unwrap_or_default();
        if !entry.evidence.is_empty() {
            if !detail.is_empty() {
                detail.push(' ');
            }
            detail.push_str(&format!("Evidence: {}.", entry.evidence.join(", ")));
        }
        match entry.status {
            status if is_failure(status) => {
                out.push_str(&format!(
                    "    <failure message=\"{}\" type=\"{}\">{}</failure>\n",
                    escape(&failure_message(status, &entry.subject.to_string())),
                    escape(status.as_str()),
                    escape(&detail)
                ));
            },
            VerificationStatus::PartiallyVerified => {
                if !detail.is_empty() {
                    out.push_str(&format!(
                        "    <system-out>{}</system-out>\n",
                        escape(&detail)
                    ));
                }
            },
            VerificationStatus::Unknown => {
                out.push_str(&format!(
                    "    <skipped message=\"{}\"/>\n",
                    escape(&if detail.is_empty() {
                        "the question could not be asked".to_owned()
                    } else {
                        detail
                    })
                ));
            },
            _ => {
                if !detail.is_empty() {
                    out.push_str(&format!(
                        "    <system-out>{}</system-out>\n",
                        escape(&detail)
                    ));
                }
            },
        }
        out.push_str("  </testcase>\n");
    }

    for error in &report.sections.errors {
        render_error(error, &mut out);
    }

    out.push_str("</testsuite>\n</testsuites>\n");
    Ok(out)
}

/// Whether a verification status should fail a build.
///
/// `UNVERIFIED` and `CONFLICTING` both fail. Treating `UNVERIFIED` as a pass is how an
/// artifact with nothing behind it ships, and `CONFLICTING` is the outcome that must
/// never pass silently.
#[must_use]
pub const fn is_failure(status: VerificationStatus) -> bool {
    matches!(
        status,
        VerificationStatus::Unverified | VerificationStatus::Conflicting
    )
}

/// The message a failing case carries.
fn failure_message(status: VerificationStatus, subject: &str) -> String {
    match status {
        VerificationStatus::Conflicting => format!(
            "{subject}: the evidence contradicts the claim. This is not an absence of \
             evidence but evidence pointing the other way, and it must be resolved before \
             the artifact is treated as verified."
        ),
        VerificationStatus::Unverified => format!(
            "{subject}: no evidence supports the claim. This is not a statement that the \
             claim is false, only that nothing here establishes it."
        ),
        _ => format!("{subject}: {status}"),
    }
}

/// Renders one failure from the errors section as an `<error>` case.
fn render_error(error: &ReportError, out: &mut String) {
    out.push_str(&format!(
        "  <testcase name=\"{}\" classname=\"{}\">\n    <error message=\"{}\" \
         type=\"{}\"/>\n  </testcase>\n",
        escape(&error.code),
        escape(&error.category),
        escape(&error.message),
        escape(&error.category)
    ));
}

/// Escapes a string for an XML attribute or text node.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML has no representation for these control characters at all, and
            // emitting one produces a document no parser will read.
            c if c.is_control() && c != '\n' && c != '\t' => {},
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ReportError, VerificationEntry};
    use amasario_core::{EntityKind, EntityRef};

    fn subject(id: &str) -> EntityRef {
        EntityRef::new(EntityKind::Contract, id).expect("a reference")
    }

    fn report_with(status: VerificationStatus) -> Report {
        let mut report = Report::new(subject("C-subject"));
        report.add_verification(VerificationEntry::new(subject("C-subject"), status));
        report
    }

    #[test]
    fn a_conflicting_claim_fails_the_build() {
        // The status exists because a claimed revision that rebuilds to a different
        // digest must not be reported as verified, and a CI gate is where that matters
        // most.
        let rendered = render(&report_with(VerificationStatus::Conflicting)).expect("renders");
        assert!(rendered.contains("<failure"), "a conflict must fail");
        assert!(rendered.contains("contradicts the claim"));
        assert!(rendered.contains("failures=\"1\""));
    }

    #[test]
    fn an_unverified_claim_fails_the_build() {
        // Treating UNVERIFIED as a pass is how an artifact with nothing behind it ships.
        let rendered = render(&report_with(VerificationStatus::Unverified)).expect("renders");
        assert!(rendered.contains("<failure"));
        assert!(rendered.contains("no evidence supports the claim"));
    }

    #[test]
    fn a_verified_claim_passes() {
        let rendered = render(&report_with(VerificationStatus::Verified)).expect("renders");
        assert!(!rendered.contains("<failure"));
        assert!(rendered.contains("failures=\"0\""));
    }

    #[test]
    fn an_unknown_outcome_is_skipped_rather_than_passed_or_failed() {
        // "The question could not be asked" is neither a pass nor a failure, and
        // reporting it as either would be a claim the analysis did not make.
        let rendered = render(&report_with(VerificationStatus::Unknown)).expect("renders");
        assert!(rendered.contains("<skipped"));
        assert!(rendered.contains("skipped=\"1\""));
        assert!(!rendered.contains("<failure"));
    }

    #[test]
    fn a_partial_verification_passes_but_says_what_it_did_not_establish() {
        let mut report = Report::new(subject("C-subject"));
        report.add_verification(
            VerificationEntry::new(subject("C-subject"), VerificationStatus::PartiallyVerified)
                .with_detail("the digest matches; the build configuration was not attested"),
        );
        let rendered = render(&report).expect("renders");
        assert!(!rendered.contains("<failure"));
        assert!(rendered.contains("the build configuration was not attested"));
    }

    #[test]
    fn a_run_that_could_not_complete_is_an_error_not_a_failure() {
        // "The network did not answer" and "the claim is contradicted" are different
        // findings, and JUnit has a separate element for each.
        let mut report = Report::new(subject("C-subject"));
        report.add_error(ReportError {
            code: "AMASARIO_NETWORK_TRANSIENT".to_owned(),
            category: "NETWORK".to_owned(),
            message: "the endpoint timed out".to_owned(),
            retryable: Some(true),
            path: None,
            detail: None,
            cause: None,
            boundary: None,
        });
        let rendered = render(&report).expect("renders");
        assert!(rendered.contains("<error"));
        assert!(rendered.contains("errors=\"1\""));
        assert!(!rendered.contains("<failure"));
    }

    #[test]
    fn evidence_is_carried_into_the_output() {
        let mut report = Report::new(subject("C-subject"));
        report.add_verification(
            VerificationEntry::new(subject("C-subject"), VerificationStatus::Verified)
                .with_evidence(vec!["ev-1".to_owned()]),
        );
        let rendered = render(&report).expect("renders");
        assert!(rendered.contains("Evidence: ev-1."));
    }

    #[test]
    fn markup_in_a_subject_does_not_produce_an_unparseable_document() {
        let mut report = Report::new(subject("C-subject"));
        report.add_verification(VerificationEntry::new(
            subject("C-<script>&\"x\""),
            VerificationStatus::Verified,
        ));
        let rendered = render(&report).expect("renders");
        assert!(!rendered.contains("<script>"));
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(rendered.contains("&amp;"));
        assert!(rendered.contains("&quot;"));
    }

    #[test]
    fn a_control_character_is_dropped_rather_than_emitted() {
        // XML cannot represent one at all, and emitting it produces a document no
        // parser will read.
        assert_eq!(escape("a\u{7}b"), "ab");
        assert_eq!(escape("a\nb"), "a\nb");
    }
}
