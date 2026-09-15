//! The report document, and the rules that keep its sections apart.
//!
//! # The one requirement that shapes everything here
//!
//! `report.schema.json` states its own central requirement: "observed facts, inferred
//! relationships, verification outcomes, confidence, unknown information and errors
//! occupy distinct sections and can never be merged. This is what stops a report from
//! presenting an inference in the same voice as an observation, which is the failure
//! mode that makes provenance tooling untrustworthy."
//!
//! That is why [`Statement`] and [`InferredStatement`] are two types rather than one
//! with a flag. The schema defines them separately - and repeats the field list in
//! `inferredStatement` rather than composing it with `$ref` - explicitly "so that
//! `inferenceBasis` is structurally required here and structurally rejected on an
//! observed fact". A single type with an optional basis would make the requirement
//! unenforceable the moment someone forgot to set it.
//!
//! # Every section is present, even when empty
//!
//! The schema requires all six section arrays. "An absent section and an empty section
//! mean different things, and only one of them is honest": an empty `unknown` array
//! says the analysis answered everything it asked, while a missing one is ambiguous
//! between that and a producer that forgot to report what it could not determine.
//! [`Sections`] therefore gives every array a default of empty rather than making any
//! of them optional.
//!
//! # The disclaimer is not optional either
//!
//! `disclaimers` has `minItems: 1`, and rule `provenance/evidence-traceability`
//! requires the non-security-determination statement to be present "so that an
//! implementation cannot emit a report that reads as a security assessment".
//! [`DISCLAIMERS`] is applied by [`Report::new`] and cannot be removed through the
//! builder, because a report without it is a report the specification refuses.

use std::fmt;

use amasario_core::{
    API_VERSION, Confidence, EngineError, EntityRef, ObservationBoundary, Result, SPEC_VERSION,
    VerificationStatus,
};
use amasario_graph::{EdgeDocument, GraphDocument};
use serde::{Deserialize, Serialize};

/// The statements every report must carry.
///
/// The first is the one rule `provenance/evidence-traceability` requires. It is
/// written to be read by a person who has just been handed a report and might
/// otherwise read it as an assessment of the contracts it describes.
pub const DISCLAIMERS: [&str; 2] = [
    "Amasario is dependency, provenance and impact infrastructure. It is not a security \
     scanner, and nothing in this report is a security assessment. No statement here means \
     that a contract is secure, safe, malicious or vulnerable.",
    "'VERIFIED' states that the evidence is consistent with the claim it is attached to. It \
     states nothing about the trustworthiness of the entity the claim concerns. A fully \
     verified provenance chain can describe a deliberate backdoor.",
];

/// The shortest a statement may be.
///
/// Enforced rather than ignored, because the schema enforces it: a producer that emitted
/// a shorter one would generate a document it could not itself read back.
pub const MINIMUM_STATEMENT_LENGTH: usize = 8;
/// The longest a statement may be.
pub const MAXIMUM_STATEMENT_LENGTH: usize = 4096;
/// The shortest an explanation or inference basis may be.
pub const MINIMUM_EXPLANATION_LENGTH: usize = 8;
/// The longest an explanation may be.
pub const MAXIMUM_EXPLANATION_LENGTH: usize = 4096;
/// The longest an inference basis may be.
pub const MAXIMUM_BASIS_LENGTH: usize = 2048;
/// The shortest a question may be.
pub const MINIMUM_QUESTION_LENGTH: usize = 8;
/// The longest a question or a detail may be.
pub const MAXIMUM_QUESTION_LENGTH: usize = 2048;
/// The shortest a disclaimer may be.
pub const MINIMUM_DISCLAIMER_LENGTH: usize = 16;
/// The longest a disclaimer may be.
pub const MAXIMUM_DISCLAIMER_LENGTH: usize = 2048;

/// Why a question could not be answered.
///
/// The schema's `unknown.reason` enumeration. A free-text reason would let a report say
/// "we could not tell" without saying whether the network failed, the contract was
/// absent or the question was never answerable - which is the distinction that decides
/// whether a consumer retries, gives up, or changes the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum UnknownReason {
    /// The entity or record does not exist at the stated boundary.
    NotFound,
    /// A request failed at the transport level.
    NetworkError,
    /// A request exceeded its deadline.
    Timeout,
    /// A response arrived and could not be interpreted.
    MalformedResponse,
    /// The answer lies outside the observation boundary that was declared.
    OutOfBoundary,
    /// Answering would require something the engine is not permitted to do.
    NotPermitted,
    /// The question cannot be answered by any observation this engine can make.
    Unsupported,
    /// The evidence needed exists but was not available to this run.
    EvidenceUnavailable,
    /// No evidence class in the specification can answer this question.
    NoEvidenceTypeCanAnswer,
}

impl UnknownReason {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "NOT_FOUND",
            Self::NetworkError => "NETWORK_ERROR",
            Self::Timeout => "TIMEOUT",
            Self::MalformedResponse => "MALFORMED_RESPONSE",
            Self::OutOfBoundary => "OUT_OF_BOUNDARY",
            Self::NotPermitted => "NOT_PERMITTED",
            Self::Unsupported => "UNSUPPORTED",
            Self::EvidenceUnavailable => "EVIDENCE_UNAVAILABLE",
            Self::NoEvidenceTypeCanAnswer => "NO_EVIDENCE_TYPE_CAN_ANSWER",
        }
    }

    /// Every reason, in the order the schema lists them.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::NotFound,
            Self::NetworkError,
            Self::Timeout,
            Self::MalformedResponse,
            Self::OutOfBoundary,
            Self::NotPermitted,
            Self::Unsupported,
            Self::EvidenceUnavailable,
            Self::NoEvidenceTypeCanAnswer,
        ]
    }

    /// Whether a later run could plausibly answer the question.
    ///
    /// The distinction a consumer needs before scheduling a retry, and it is not
    /// derivable from the reason's name: `NOT_FOUND` is a fact about the chain that a
    /// retry will not change, whereas `TIMEOUT` is a fact about this run.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::NetworkError | Self::Timeout | Self::EvidenceUnavailable
        )
    }
}

impl fmt::Display for UnknownReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One statement about something observed, with the evidence that supports it.
///
/// `evidence` is required and non-empty by the schema: "a report statement without
/// evidence is an unsupported assertion, which is the thing this specification exists
/// to prevent".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Statement {
    /// The statement, stated factually.
    pub statement: String,
    /// The entity the statement is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityRef>,
    /// The evidence supporting it. Never empty.
    pub evidence: Vec<String>,
    /// How strongly the evidence supports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
}

impl Statement {
    /// Builds a statement, refusing one that cites no evidence.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the statement is shorter than the schema's
    /// minimum or cites no evidence.
    pub fn new(
        statement: impl Into<String>,
        entity: Option<EntityRef>,
        evidence: Vec<String>,
    ) -> Result<Self> {
        let statement = statement.into();
        if statement.len() < MINIMUM_STATEMENT_LENGTH {
            return Err(EngineError::Validation {
                path: "/sections/observedFacts/statement".to_owned(),
                detail: format!(
                    "a statement must be at least {MINIMUM_STATEMENT_LENGTH} characters; \
                     got {}",
                    statement.len()
                ),
            });
        }
        if evidence.is_empty() {
            return Err(EngineError::Report(
                "a statement without evidence is an unsupported assertion, which is the \
                 thing the specification exists to prevent"
                    .to_owned(),
            ));
        }
        Ok(Self {
            statement,
            entity,
            evidence,
            confidence: None,
        })
    }

    /// Attaches a confidence, which carries its own evidence.
    #[must_use]
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Reports why this statement is not structurally valid.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        check_length(
            &mut failures,
            "observedFacts.statement",
            &self.statement,
            MINIMUM_STATEMENT_LENGTH,
            MAXIMUM_STATEMENT_LENGTH,
        );
        if self.evidence.is_empty() {
            failures.push("observedFacts.statement cites no evidence".to_owned());
        }
        failures
    }
}

/// One statement about a relationship that was derived rather than observed.
///
/// Identical to [`Statement`] except that `inference_basis` is required. The two are
/// separate types because the schema defines them separately, so that a basis cannot be
/// omitted from an inference and cannot be attached to an observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InferredStatement {
    /// The statement, stated as an inference.
    pub statement: String,
    /// The entity the statement is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityRef>,
    /// The evidence supporting the inference. Never empty.
    pub evidence: Vec<String>,
    /// How strongly the evidence supports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// Why the relationship was inferred rather than observed.
    pub inference_basis: String,
}

impl InferredStatement {
    /// Builds an inferred statement, refusing one with no basis or no evidence.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the statement or the basis is outside the
    /// schema's length bounds, or when no evidence is cited.
    pub fn new(
        statement: impl Into<String>,
        entity: Option<EntityRef>,
        evidence: Vec<String>,
        inference_basis: impl Into<String>,
    ) -> Result<Self> {
        let statement = statement.into();
        let inference_basis = inference_basis.into();
        if statement.len() < MINIMUM_STATEMENT_LENGTH {
            return Err(EngineError::Validation {
                path: "/sections/inferredRelationships/statement".to_owned(),
                detail: format!(
                    "an inferred statement must be at least {MINIMUM_STATEMENT_LENGTH} \
                     characters; got {}",
                    statement.len()
                ),
            });
        }
        if inference_basis.len() < MINIMUM_EXPLANATION_LENGTH {
            return Err(EngineError::Report(
                "an inferred statement must state the basis of the inference, so that it \
                 cannot be presented in the same voice as an observation"
                    .to_owned(),
            ));
        }
        if evidence.is_empty() {
            return Err(EngineError::Report(
                "an inference must cite the evidence of the inputs it was drawn from".to_owned(),
            ));
        }
        Ok(Self {
            statement,
            entity,
            evidence,
            confidence: None,
            inference_basis,
        })
    }

    /// Attaches a confidence, which carries its own evidence.
    #[must_use]
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Reports why this statement is not structurally valid.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        check_length(
            &mut failures,
            "inferredRelationships.statement",
            &self.statement,
            MINIMUM_STATEMENT_LENGTH,
            MAXIMUM_STATEMENT_LENGTH,
        );
        check_length(
            &mut failures,
            "inferredRelationships.inferenceBasis",
            &self.inference_basis,
            MINIMUM_EXPLANATION_LENGTH,
            MAXIMUM_BASIS_LENGTH,
        );
        if self.evidence.is_empty() {
            failures.push("inferredRelationships.statement cites no evidence".to_owned());
        }
        failures
    }
}

/// The verification outcome for one subject that was checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationEntry {
    /// The subject that was checked.
    pub subject: EntityRef,
    /// The outcome.
    pub status: VerificationStatus,
    /// What the check found, where it needs saying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The evidence the outcome rests on.
    ///
    /// An empty list is omitted and a missing list reads as empty, so that the document
    /// round-trips: a producer that writes an omission must accept the omission back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

impl VerificationEntry {
    /// Builds an entry.
    #[must_use]
    pub const fn new(subject: EntityRef, status: VerificationStatus) -> Self {
        Self {
            subject,
            status,
            detail: None,
            evidence: Vec::new(),
        }
    }

    /// Attaches the finding in prose.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attaches the evidence the outcome rests on.
    #[must_use]
    pub fn with_evidence(mut self, evidence: Vec<String>) -> Self {
        self.evidence = evidence;
        self
    }
}

/// A confidence assigned in the report, with the evidence it derives from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfidenceEntry {
    /// The subject the confidence is about.
    pub subject: EntityRef,
    /// The confidence, which carries its own evidence.
    pub confidence: Confidence,
}

/// A question the analysis could not answer.
///
/// This section exists "so that a report can be complete about its own incompleteness
/// instead of omitting what it could not determine".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnknownEntry {
    /// The question, stated as a question.
    pub question: String,
    /// Why it could not be answered.
    pub reason: UnknownReason,
    /// What happened, in more detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl UnknownEntry {
    /// Builds an entry.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the question is outside the schema's bounds,
    /// which matters because the schema says the question must be *stated as a
    /// question* and a five-character fragment is not one.
    pub fn new(question: impl Into<String>, reason: UnknownReason) -> Result<Self> {
        let question = question.into();
        if question.len() < MINIMUM_QUESTION_LENGTH {
            return Err(EngineError::Validation {
                path: "/sections/unknown/question".to_owned(),
                detail: format!(
                    "a question must be at least {MINIMUM_QUESTION_LENGTH} characters; got {}",
                    question.len()
                ),
            });
        }
        if question.len() > MAXIMUM_QUESTION_LENGTH {
            return Err(EngineError::Validation {
                path: "/sections/unknown/question".to_owned(),
                detail: format!(
                    "a question must be at most {MAXIMUM_QUESTION_LENGTH} characters; got {}",
                    question.len()
                ),
            });
        }
        Ok(Self {
            question,
            reason,
            detail: None,
        })
    }

    /// Attaches the detail.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A structured failure, in the shape `error.schema.json` defines.
///
/// `code`, `category` and `message` are required by the schema, and `code` and
/// `category` are required precisely because "a consumer that has only the message
/// cannot branch reliably".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportError {
    /// The stable machine-readable code.
    pub code: String,
    /// The failure domain.
    pub category: String,
    /// A human-readable explanation.
    pub message: String,
    /// Whether retrying could plausibly succeed. Absent means unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    /// The JSON pointer to the offending value, for validation failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Structured detail. Never a secret, a credential or a private key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    /// The underlying failure, when this one wraps another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<Self>>,
    /// The boundary at which the failure occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
}

impl ReportError {
    /// Projects an engine error into its published form.
    ///
    /// The category and the code come from the error itself rather than from a second
    /// table here, so a report and a CLI cannot disagree about what kind of failure
    /// occurred or what to call it.
    #[must_use]
    pub fn of(error: &EngineError) -> Self {
        Self {
            code: error.code(),
            category: error.category().as_str().to_owned(),
            message: error.to_string(),
            // `None` rather than `false` for a failure that is not a network error,
            // because the schema says an absent `retryable` means unknown and "must not
            // be read as false".
            retryable: None,
            path: match error {
                EngineError::Validation { path, .. } => Some(path.clone()),
                _ => None,
            },
            detail: error.detail(),
            cause: None,
            boundary: None,
        }
    }

    /// Projects a network failure, which is the one case where retryability is known.
    #[must_use]
    pub const fn with_retryability(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }
}

/// An explanation of why a relationship in the report exists.
///
/// Required so that "a reader does not have to reconstruct the reasoning from the
/// evidence set". `observationally_supported` is on the finding itself rather than left
/// to the section it appears in, because "a relationship explained only by inference
/// must say so on the finding itself".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelationshipFinding {
    /// The relationship being explained.
    pub relationship: amasario_core::Relationship,
    /// The edge's identifier, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_id: Option<String>,
    /// Why the relationship exists.
    pub explanation: String,
    /// Whether the relationship was directly observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observationally_supported: Option<bool>,
}

impl RelationshipFinding {
    /// Builds a finding, refusing an explanation that is too short to explain anything.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the explanation is outside the schema's bounds.
    pub fn new(
        relationship: amasario_core::Relationship,
        explanation: impl Into<String>,
    ) -> Result<Self> {
        let explanation = explanation.into();
        check_explanation(&explanation)?;
        Ok(Self {
            relationship,
            edge_id: None,
            explanation,
            observationally_supported: None,
        })
    }

    /// Derives a finding from an edge document.
    ///
    /// The explanation is written from the edge's own facts - the relationship, whether
    /// it was observed, and the evidence it cites - so that a reader is told why the
    /// engine believes the edge rather than merely that it does.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the derived explanation is outside the schema's
    /// bounds, which cannot happen for an edge that carries a relationship.
    pub fn of_edge(edge: &EdgeDocument) -> Result<Self> {
        let how = if edge.observed {
            "directly observed"
        } else {
            "inferred rather than observed, and is reported as such"
        };
        let explanation = format!(
            "{} is held between {} and {} because the relationship was {how}, on the \
             strength of {} evidence record(s) cited by this report",
            edge.relationship.as_str(),
            edge.source,
            edge.target,
            edge.evidence.len()
        );
        check_explanation(&explanation)?;
        Ok(Self {
            relationship: edge.relationship,
            edge_id: Some(edge.id.clone()),
            explanation,
            observationally_supported: Some(edge.observed),
        })
    }

    /// Attaches the edge identifier.
    #[must_use]
    pub fn with_edge_id(mut self, id: impl Into<String>) -> Self {
        self.edge_id = Some(id.into());
        self
    }

    /// Records whether the relationship was directly observed.
    #[must_use]
    pub const fn observed(mut self, observed: bool) -> Self {
        self.observationally_supported = Some(observed);
        self
    }
}

/// The report body, separated by epistemic status.
///
/// Every array is required by the schema and defaults to empty here, so a report cannot
/// be published with a section missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sections {
    /// Statements directly supported by evidence, with no inference.
    #[serde(default)]
    pub observed_facts: Vec<Statement>,
    /// Relationships that were derived rather than observed.
    #[serde(default)]
    pub inferred_relationships: Vec<InferredStatement>,
    /// The verification outcome for each subject that was checked.
    #[serde(default)]
    pub verification: Vec<VerificationEntry>,
    /// Confidence values assigned, with their evidence.
    #[serde(default)]
    pub confidence: Vec<ConfidenceEntry>,
    /// Questions the analysis could not answer.
    #[serde(default)]
    pub unknown: Vec<UnknownEntry>,
    /// Failures encountered during the analysis.
    #[serde(default)]
    pub errors: Vec<ReportError>,
}

impl Sections {
    /// How many entries the report carries in total.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.observed_facts.len()
            + self.inferred_relationships.len()
            + self.verification.len()
            + self.confidence.len()
            + self.unknown.len()
            + self.errors.len()
    }

    /// Whether every section is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The report document, in the shape `report.schema.json` defines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    /// The major-version compatibility family.
    pub api_version: String,
    /// The normative specification version that produced the report.
    pub spec_version: String,
    /// What the report is about.
    pub target: EntityRef,
    /// The observation boundary the reported analysis was made at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// The report body.
    pub sections: Sections,
    /// Why each important relationship exists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationship_findings: Vec<RelationshipFinding>,
    /// Statements that must accompany the report. Never empty.
    pub disclaimers: Vec<String>,
    /// Whether any part of the reported analysis was bounded before exhaustion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// When the report was generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// The graph the report was assembled from, held in memory only.
    ///
    /// `report.schema.json` has no field for a graph, so this is skipped in both
    /// directions: it is not published, and reading a document back does not restore
    /// it. It exists because the DOT renderer needs the topology, and a DOT rendering
    /// of relationship findings alone would be a picture of the explanations rather
    /// than of the graph.
    #[serde(skip)]
    pub graph: Option<GraphDocument>,
}

impl Report {
    /// Builds an empty report for a target, carrying the required disclaimers.
    #[must_use]
    pub fn new(target: EntityRef) -> Self {
        Self {
            api_version: API_VERSION.to_owned(),
            spec_version: SPEC_VERSION.to_owned(),
            target,
            boundary: None,
            sections: Sections::default(),
            relationship_findings: Vec::new(),
            disclaimers: DISCLAIMERS.iter().map(|line| (*line).to_owned()).collect(),
            truncated: None,
            generated_at: None,
            graph: None,
        }
    }

    /// Records the observation boundary the analysis was made at.
    #[must_use]
    pub fn with_boundary(mut self, boundary: ObservationBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Records when the report was generated.
    #[must_use]
    pub fn generated_at(mut self, timestamp: impl Into<String>) -> Self {
        self.generated_at = Some(timestamp.into());
        self
    }

    /// Records whether the analysis behind the report was bounded.
    #[must_use]
    pub const fn truncated(mut self, truncated: bool) -> Self {
        self.truncated = Some(truncated);
        self
    }

    /// Attaches the graph, in memory only.
    #[must_use]
    pub fn with_graph(mut self, graph: GraphDocument) -> Self {
        self.graph = Some(graph);
        self
    }

    /// Adds an observed fact.
    pub fn add_observed(&mut self, statement: Statement) {
        self.sections.observed_facts.push(statement);
    }

    /// Adds an inference.
    pub fn add_inferred(&mut self, statement: InferredStatement) {
        self.sections.inferred_relationships.push(statement);
    }

    /// Adds a verification outcome.
    pub fn add_verification(&mut self, entry: VerificationEntry) {
        self.sections.verification.push(entry);
    }

    /// Adds a confidence.
    pub fn add_confidence(&mut self, entry: ConfidenceEntry) {
        self.sections.confidence.push(entry);
    }

    /// Adds a question the analysis could not answer.
    pub fn add_unknown(&mut self, entry: UnknownEntry) {
        self.sections.unknown.push(entry);
    }

    /// Adds a failure.
    pub fn add_error(&mut self, error: ReportError) {
        self.sections.errors.push(error);
    }

    /// Records the failure behind a bounded analysis.
    ///
    /// A report that says it was truncated without saying why is a report whose reader
    /// cannot tell a depth limit from a rate limit.
    pub fn add_error_of(&mut self, error: &EngineError) {
        self.sections.errors.push(ReportError::of(error));
    }

    /// Adds an explanation of a relationship.
    pub fn add_relationship_finding(&mut self, finding: RelationshipFinding) {
        self.relationship_findings.push(finding);
    }

    /// The subjects the report assigns a confidence to.
    #[must_use]
    pub fn confident_subjects(&self) -> Vec<&EntityRef> {
        self.sections
            .confidence
            .iter()
            .map(|entry| &entry.subject)
            .collect()
    }

    /// Reports every structural problem that would make this document invalid.
    ///
    /// The schema's constraints restated over the values the engine holds, so that a
    /// producer can check before publishing rather than discover afterwards that a
    /// consumer refused its report. Each failure names the section and the rule.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();

        if self.api_version != API_VERSION {
            failures.push(format!(
                "apiVersion is {} and this engine emits {API_VERSION}",
                self.api_version
            ));
        }
        if self.spec_version != SPEC_VERSION {
            failures.push(format!(
                "specVersion is {} and this engine emits {SPEC_VERSION}",
                self.spec_version
            ));
        }
        if self.disclaimers.is_empty() {
            failures.push(
                "disclaimers is empty, and report.schema.json requires at least one".to_owned(),
            );
        }
        for (index, disclaimer) in self.disclaimers.iter().enumerate() {
            check_length(
                &mut failures,
                &format!("disclaimers[{index}]"),
                disclaimer,
                MINIMUM_DISCLAIMER_LENGTH,
                MAXIMUM_DISCLAIMER_LENGTH,
            );
        }

        for statement in &self.sections.observed_facts {
            failures.extend(statement.failures());
        }
        for statement in &self.sections.inferred_relationships {
            failures.extend(statement.failures());
        }

        for (index, entry) in self.sections.unknown.iter().enumerate() {
            check_length(
                &mut failures,
                &format!("unknown[{index}].question"),
                &entry.question,
                MINIMUM_QUESTION_LENGTH,
                MAXIMUM_QUESTION_LENGTH,
            );
        }

        for (index, entry) in self.sections.confidence.iter().enumerate() {
            if entry.confidence.evidence.is_empty() {
                failures.push(format!(
                    "confidence[{index}] names a level with no evidence, which the \
                     specification forbids at every level including UNKNOWN"
                ));
            }
        }

        for (index, finding) in self.relationship_findings.iter().enumerate() {
            check_length(
                &mut failures,
                &format!("relationshipFindings[{index}].explanation"),
                &finding.explanation,
                MINIMUM_EXPLANATION_LENGTH,
                MAXIMUM_EXPLANATION_LENGTH,
            );
        }

        for (index, error) in self.sections.errors.iter().enumerate() {
            if error.code.is_empty() {
                failures.push(format!("errors[{index}].code is empty"));
            }
            if error.category.is_empty() {
                failures.push(format!("errors[{index}].category is empty"));
            }
            if error.message.len() < 4 {
                failures.push(format!(
                    "errors[{index}].message is shorter than the schema's minimum of 4"
                ));
            }
        }

        failures
    }

    /// Fails if the report is not structurally valid.
    ///
    /// # Errors
    ///
    /// Returns a report error summarising the first problem found.
    pub fn validate(&self) -> Result<()> {
        let failures = self.failures();
        if failures.is_empty() {
            return Ok(());
        }
        Err(EngineError::Report(format!(
            "{} problem(s) found; first: {}",
            failures.len(),
            failures[0]
        )))
    }

    /// The report's canonical JSON.
    ///
    /// # Errors
    ///
    /// Returns a report error when the document cannot be serialised, and a validation
    /// error when it would be structurally invalid - checked *before* serialising, so
    /// that an invalid report is never produced at all.
    pub fn canonical_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            EngineError::Report(format!("the report could not be written: {error}"))
        })
    }

    /// The report's JSON, indented for a person to read.
    ///
    /// # Errors
    ///
    /// As [`Report::canonical_json`].
    pub fn pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|error| {
            EngineError::Report(format!("the report could not be written: {error}"))
        })
    }
}

/// Records a length problem, if there is one.
fn check_length(
    failures: &mut Vec<String>,
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) {
    if value.len() < minimum {
        failures.push(format!(
            "{field} is {} characters, shorter than the schema's minimum of {minimum}",
            value.len()
        ));
    }
    if value.len() > maximum {
        failures.push(format!(
            "{field} is {} characters, longer than the schema's maximum of {maximum}",
            value.len()
        ));
    }
}

/// Checks an explanation against the schema's bounds.
fn check_explanation(explanation: &str) -> Result<()> {
    let mut failures = Vec::new();
    check_length(
        &mut failures,
        "relationshipFindings.explanation",
        explanation,
        MINIMUM_EXPLANATION_LENGTH,
        MAXIMUM_EXPLANATION_LENGTH,
    );
    if let Some(first) = failures.first() {
        return Err(EngineError::Validation {
            path: "/relationshipFindings/explanation".to_owned(),
            detail: first.clone(),
        });
    }
    Ok(())
}
