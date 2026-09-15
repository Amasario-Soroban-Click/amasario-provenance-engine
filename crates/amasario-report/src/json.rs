//! The JSON renderer: the specification's report document, canonically written.
//!
//! Nothing is added here. The document is the model, and adding a convenience field
//! would make the output invalid against `report.schema.json` with
//! `additionalProperties: false` - which is how the engine came to emit documents that
//! consumers refused.
//!
//! Canonical means the field order is the schema's, the section arrays are in the order
//! they were added, and an absent optional field is omitted rather than written as
//! `null`, so that "not established" and "established as empty" cannot be confused.

use amasario_core::Result;

use crate::model::Report;

/// Renders a report as canonical JSON.
///
/// # Errors
///
/// Returns a report error when the report is structurally invalid or cannot be
/// serialised.
pub fn render(report: &Report) -> Result<String> {
    report.canonical_json()
}

/// Renders a report as indented JSON.
///
/// # Errors
///
/// As [`render`].
pub fn render_pretty(report: &Report) -> Result<String> {
    report.pretty_json()
}
