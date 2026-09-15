//! The output formats, and the one place that knows which renderer each names.
//!
//! # Why the format is a value rather than a branch in the CLI
//!
//! A caller that has a [`Format`] can print the list of what it accepts, parse one from
//! a command line, and name a file extension without the CLI growing a second table of
//! the same information. Report generation is a library concern here; the CLI's job is
//! to obtain a format and a destination.
//!
//! # What every renderer is required to do
//!
//! Each renderer keeps the specification's sections apart. That is not a stylistic
//! choice - `report.schema.json` states that merging them "is the failure mode that
//! makes provenance tooling untrustworthy" - and the Markdown, DOT and JUnit renderers
//! are held to it as strictly as the JSON one. Concretely: no renderer may print an
//! inference in the same block or under the same heading as an observation, and no
//! renderer may omit the disclaimers, because a rendered report is the form a reader
//! actually sees.

use amasario_core::{EngineError, Result};

use crate::dot;
use crate::json;
use crate::junit;
use crate::markdown;
use crate::model::Report;
use crate::summary;

/// A format a report can be rendered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Format {
    /// The specification's report document, as canonical JSON.
    Json,
    /// A readable rendering that keeps the sections apart.
    Markdown,
    /// The dependency graph in the Graphviz DOT language.
    Dot,
    /// A JUnit XML testsuite, for a CI job to consume.
    Junit,
}

impl Format {
    /// The stable name, used on the command line and in output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Dot => "dot",
            Self::Junit => "junit",
        }
    }

    /// Every format, in a stable order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Json, Self::Markdown, Self::Dot, Self::Junit]
    }

    /// The customary file extension, without a dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
            Self::Dot => "dot",
            Self::Junit => "xml",
        }
    }

    /// Parses a format name.
    ///
    /// Accepts the customary aliases a person types rather than only the canonical
    /// name, because refusing `md` for Markdown would be pedantry with no benefit.
    ///
    /// # Errors
    ///
    /// Returns a configuration error naming the formats that are accepted.
    pub fn parse(name: &str) -> Result<Self> {
        let normalised = name.trim().to_ascii_lowercase();
        let found = match normalised.as_str() {
            "json" => Some(Self::Json),
            "markdown" | "md" => Some(Self::Markdown),
            "dot" | "graphviz" => Some(Self::Dot),
            "junit" | "junitxml" | "xml" => Some(Self::Junit),
            _ => None,
        };
        found.ok_or_else(|| {
            let accepted: Vec<&str> = Self::all().iter().map(|format| format.as_str()).collect();
            EngineError::Configuration(format!(
                "unknown report format {name:?}; accepted formats are {}",
                accepted.join(", ")
            ))
        })
    }

    /// Every name this format accepts, for help text.
    #[must_use]
    pub const fn accepted_names(self) -> &'static [&'static str] {
        match self {
            Self::Json => &["json"],
            Self::Markdown => &["markdown", "md"],
            Self::Dot => &["dot", "graphviz"],
            Self::Junit => &["junit", "xml"],
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Renders a report in the requested format.
///
/// # Errors
///
/// Returns a report error when the report is structurally invalid - checked by the JSON
/// renderer, and by [`Report::validate`] for every other format first - and a report
/// error when a renderer fails.
pub fn render(report: &Report, format: Format) -> Result<String> {
    // Every format is validated, not only the JSON one. A Markdown report of an invalid
    // document would be just as misleading as the document itself, and harder to notice.
    report.validate()?;
    match format {
        Format::Json => json::render(report),
        Format::Markdown => markdown::render(report),
        Format::Dot => dot::render(report),
        Format::Junit => junit::render(report),
    }
}

/// Renders a one-paragraph summary of a report.
///
/// # Errors
///
/// As [`render`].
pub fn render_summary(report: &Report) -> Result<String> {
    report.validate()?;
    Ok(summary::render(report))
}
