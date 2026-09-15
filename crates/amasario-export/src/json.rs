//! The JSON export.
//!
//! The graph document is already the JSON the specification defines, so this renderer
//! adds nothing to it and removes nothing from it. That is the point: an export that
//! reshaped the document would produce a file that is no longer the thing the schemas
//! describe, and a consumer that validated one would reject the other.
//!
//! JSON is the format with no loss. An edge keeps its basis, its classes and its
//! verification status here, all of which `graph.schema.json` omits from a published
//! edge and which GraphML and DOT have no way to carry.

use amasario_core::Result;
use amasario_graph::GraphDocument;

use crate::errors::ExportFailure;

/// Renders a graph document as canonical JSON.
///
/// # Errors
///
/// Returns an export error when the document cannot be serialised, which for this shape
/// would be a defect in the engine.
pub fn render(graph: &GraphDocument) -> Result<String> {
    graph.canonical_json()
}

/// Renders a graph document as indented JSON.
///
/// # Errors
///
/// As [`render`].
pub fn render_pretty(graph: &GraphDocument) -> Result<String> {
    graph.pretty_json()
}

/// Renders any serialisable value as canonical JSON.
///
/// Used by the CLI for values that are not graph documents - a snapshot, a diff, a
/// dependency set - so that one place decides how JSON is written and every export
/// agrees on it.
///
/// # Errors
///
/// Returns an export error when the value cannot be serialised.
pub fn render_value<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| {
        ExportFailure::Serialisation {
            format: "JSON",
            detail: error.to_string(),
        }
        .into_error()
    })
}

/// Renders any serialisable value as indented JSON.
///
/// # Errors
///
/// As [`render_value`].
pub fn render_value_pretty<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string_pretty(value).map_err(|error| {
        ExportFailure::Serialisation {
            format: "JSON",
            detail: error.to_string(),
        }
        .into_error()
    })
}
