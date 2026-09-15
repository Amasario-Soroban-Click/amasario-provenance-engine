//! The deterministic analysis pipeline.
//!
//! The pipeline fixes the order of the analysis and records what happened at each
//! stage. Its value is not that it runs the stages - the crates that own each stage
//! do that - but that the order is declared in one place, that a stage cannot be
//! skipped without the report saying so, and that a run's outcome is a value rather
//! than a sequence of side effects.
//!
//! Two properties are enforced here rather than left to callers.
//!
//! A stage that is not applicable is recorded as skipped with a reason. Omission is
//! not an option, because a consumer cannot tell an omitted stage from one that
//! produced nothing, and those mean different things. This is the same distinction
//! the specification draws between a truncation and an absence.
//!
//! A stage failure does not become an empty result. When a stage fails, the
//! pipeline stops and returns the error with the stage that produced it, so that a
//! dependency count of zero is never confused with a request that failed.

use std::fmt;
use std::time::{Duration, Instant};

use crate::context::ExecutionContext;
use crate::errors::{EngineError, Result};

/// The stages of the analysis, in the order they are performed.
///
/// The order is normative. Dependency resolution cannot precede contract
/// inspection, because a dependency is established from something observed;
/// impact analysis cannot precede graph construction, because impact traverses the
/// graph. Declaring the order here means an implementation cannot reorder stages
/// without the change being visible in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Load and validate the engine configuration.
    LoadConfiguration,
    /// Load the Amasario specification and check version compatibility.
    LoadSpecification,
    /// Validate the requested profile.
    ValidateProfile,
    /// Resolve the target contract from the requested identifier.
    ResolveTarget,
    /// Identify the network the run observes.
    IdentifyNetwork,
    /// Inspect the contract: identity, executable hash, interface, storage.
    InspectContract,
    /// Collect the evidence supporting the claims made so far.
    CollectEvidence,
    /// Discover and classify dependency relationships.
    ResolveDependencies,
    /// Build the typed graph from the resolved edges.
    BuildGraph,
    /// Verify the provenance chain from source to deployment.
    VerifyProvenance,
    /// Calculate confidence from the evidence.
    CalculateConfidence,
    /// Analyse impact from the graph and the change under consideration.
    AnalyseImpact,
    /// Capture a normalized snapshot.
    CaptureSnapshot,
    /// Compare two snapshots.
    CompareSnapshots,
    /// Generate the report.
    GenerateReport,
    /// Export machine-readable results.
    Export,
}

impl Stage {
    /// The stable name used in output and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoadConfiguration => "load-configuration",
            Self::LoadSpecification => "load-specification",
            Self::ValidateProfile => "validate-profile",
            Self::ResolveTarget => "resolve-target",
            Self::IdentifyNetwork => "identify-network",
            Self::InspectContract => "inspect-contract",
            Self::CollectEvidence => "collect-evidence",
            Self::ResolveDependencies => "resolve-dependencies",
            Self::BuildGraph => "build-graph",
            Self::VerifyProvenance => "verify-provenance",
            Self::CalculateConfidence => "calculate-confidence",
            Self::AnalyseImpact => "analyse-impact",
            Self::CaptureSnapshot => "capture-snapshot",
            Self::CompareSnapshots => "compare-snapshots",
            Self::GenerateReport => "generate-report",
            Self::Export => "export",
        }
    }

    /// Every stage, in the order they are performed.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::LoadConfiguration,
            Self::LoadSpecification,
            Self::ValidateProfile,
            Self::ResolveTarget,
            Self::IdentifyNetwork,
            Self::InspectContract,
            Self::CollectEvidence,
            Self::ResolveDependencies,
            Self::BuildGraph,
            Self::VerifyProvenance,
            Self::CalculateConfidence,
            Self::AnalyseImpact,
            Self::CaptureSnapshot,
            Self::CompareSnapshots,
            Self::GenerateReport,
            Self::Export,
        ]
    }

    /// The stages that every run performs, regardless of what was requested.
    ///
    /// Used to check that a run did not report a required stage as skipped, which
    /// would mean the pipeline silently produced output without the analysis that
    /// justifies it.
    #[must_use]
    pub const fn is_mandatory(self) -> bool {
        matches!(
            self,
            Self::LoadConfiguration
                | Self::LoadSpecification
                | Self::ResolveTarget
                | Self::IdentifyNetwork
                | Self::InspectContract
                | Self::CollectEvidence
                | Self::ResolveDependencies
                | Self::BuildGraph
                | Self::VerifyProvenance
                | Self::CalculateConfidence
                | Self::GenerateReport
        )
    }

    /// The stages performed only when the corresponding operation was requested.
    #[must_use]
    pub const fn is_conditional(self) -> bool {
        !self.is_mandatory()
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What happened at a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// The stage completed and produced a result.
    Completed {
        /// A one-line, human-readable statement of what it produced.
        summary: String,
        /// How long it took, when it was measured.
        elapsed: Option<Duration>,
    },
    /// The stage was not applicable, and why.
    ///
    /// Recorded rather than omitted, because a consumer cannot distinguish an
    /// omitted stage from one that produced nothing.
    Skipped {
        /// Why the stage did not run.
        reason: String,
    },
    /// The stage stopped early and said so.
    ///
    /// Distinct from [`StageOutcome::Completed`] because a bounded result is not a
    /// complete one, and a consumer must be able to tell them apart.
    Truncated {
        /// Why the stage stopped.
        reason: String,
        /// A one-line statement of what it did produce.
        summary: String,
    },
}

impl StageOutcome {
    /// A completed outcome without a measured duration.
    pub fn completed(summary: impl Into<String>) -> Self {
        Self::Completed {
            summary: summary.into(),
            elapsed: None,
        }
    }

    /// A completed outcome with a measured duration.
    #[must_use]
    pub fn completed_in(summary: impl Into<String>, elapsed: Duration) -> Self {
        Self::Completed {
            summary: summary.into(),
            elapsed: Some(elapsed),
        }
    }

    /// A skipped outcome.
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped {
            reason: reason.into(),
        }
    }

    /// A truncated outcome.
    pub fn truncated(reason: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::Truncated {
            reason: reason.into(),
            summary: summary.into(),
        }
    }

    /// Whether the stage ran.
    #[must_use]
    pub const fn ran(&self) -> bool {
        !matches!(self, Self::Skipped { .. })
    }

    /// Whether the stage stopped before exhausting its work.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }

    /// The outcome's summary line, when it has one.
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        match self {
            Self::Completed { summary, .. } | Self::Truncated { summary, .. } => Some(summary),
            Self::Skipped { .. } => None,
        }
    }

    /// The reason a stage was skipped or truncated.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Skipped { reason } | Self::Truncated { reason, .. } => Some(reason),
            Self::Completed { .. } => None,
        }
    }
}

/// The record of one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageReport {
    /// Which stage.
    pub stage: Stage,
    /// What happened.
    pub outcome: StageOutcome,
}

/// The record of a whole run.
///
/// Carries the stage reports and a flag for whether any stage was truncated. The
/// flag is derived from the reports rather than set independently, so the two
/// cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct PipelineReport {
    stages: Vec<StageReport>,
}

impl PipelineReport {
    /// An empty report.
    #[must_use]
    pub const fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Records a stage outcome, replacing any earlier record for the same stage.
    ///
    /// Replacing rather than appending means a stage that is retried does not
    /// appear twice, so the report lists each stage at most once and its length is
    /// bounded by the number of stages.
    pub fn record(&mut self, stage: Stage, outcome: StageOutcome) {
        self.stages.retain(|report| report.stage != stage);
        self.stages.push(StageReport { stage, outcome });
    }

    /// The recorded stages, in the order they were recorded.
    #[must_use]
    pub fn stages(&self) -> &[StageReport] {
        &self.stages
    }

    /// The outcome recorded for a stage, if it ran.
    #[must_use]
    pub fn outcome_of(&self, stage: Stage) -> Option<&StageOutcome> {
        self.stages
            .iter()
            .find(|report| report.stage == stage)
            .map(|report| &report.outcome)
    }

    /// Whether any stage reported truncation.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.stages
            .iter()
            .any(|report| report.outcome.is_truncated())
    }

    /// The stages reporting truncation.
    #[must_use]
    pub fn truncated_stages(&self) -> Vec<Stage> {
        self.stages
            .iter()
            .filter(|report| report.outcome.is_truncated())
            .map(|report| report.stage)
            .collect()
    }

    /// The mandatory stages that did not run.
    ///
    /// Should always be empty for a run that produced output. Used by the engine to
    /// refuse to emit a result whose analysis is incomplete, because a report that
    /// silently omitted a stage would present an incomplete analysis as a complete
    /// one.
    #[must_use]
    pub fn missing_mandatory_stages(&self) -> Vec<Stage> {
        Stage::all()
            .iter()
            .copied()
            .filter(|stage| stage.is_mandatory())
            .filter(|stage| {
                !self
                    .stages
                    .iter()
                    .any(|report| report.stage == *stage && report.outcome.ran())
            })
            .collect()
    }

    /// A single-line summary of the run, for a log or a report header.
    #[must_use]
    pub fn summary(&self) -> String {
        let ran = self.stages.iter().filter(|r| r.outcome.ran()).count();
        let skipped = self.stages.len() - ran;
        let truncated = self.truncated_stages().len();
        format!("{ran} stage(s) completed, {skipped} skipped, {truncated} truncated")
    }
}

/// Runs the stages of the analysis in order.
///
/// The runner owns the ordering and the reporting; the actual work is supplied by
/// the caller as a closure per stage, because the crate that implements a stage is
/// the crate that owns its dependencies. This keeps `amasario-core` free of every
/// other crate - the alternative, where core depends on the graph and impact
/// crates to call them, would make the dependency graph cyclic.
pub struct Pipeline<'a> {
    context: &'a ExecutionContext,
    report: PipelineReport,
}

impl<'a> Pipeline<'a> {
    /// A pipeline bound to a context.
    #[must_use]
    pub const fn new(context: &'a ExecutionContext) -> Self {
        Self {
            context,
            report: PipelineReport::new(),
        }
    }

    /// Runs one stage.
    ///
    /// The context is checked for cancellation before the work starts, so a
    /// cancelled run stops at a stage boundary and every stage that ran has
    /// completed. A cancellation mid-stage would leave the engine unable to say
    /// whether the stage's work took effect.
    pub fn run<T>(
        &mut self,
        stage: Stage,
        work: impl FnOnce(&ExecutionContext) -> Result<T>,
    ) -> Result<T> {
        self.context.check_cancelled()?;
        let started = Instant::now();
        match work(self.context) {
            Ok(value) => {
                self.report.record(
                    stage,
                    StageOutcome::completed_in(format!("{stage} completed"), started.elapsed()),
                );
                Ok(value)
            },
            Err(error) => {
                // The failure is recorded as a skip rather than a completion,
                // so the report never claims a stage succeeded when it did not.
                self.report.record(
                    stage,
                    StageOutcome::skipped(format!("failed after {:?}: {error}", started.elapsed())),
                );
                Err(error)
            },
        }
    }

    /// Records a stage as skipped without attempting it.
    pub fn skip(&mut self, stage: Stage, reason: impl Into<String>) {
        self.report.record(stage, StageOutcome::skipped(reason));
    }

    /// Records a stage as completed, for work performed outside [`Pipeline::run`].
    pub fn complete(&mut self, stage: Stage, summary: impl Into<String>) {
        self.report.record(stage, StageOutcome::completed(summary));
    }

    /// Records a stage as truncated.
    pub fn truncate(
        &mut self,
        stage: Stage,
        reason: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.report
            .record(stage, StageOutcome::truncated(reason, summary));
    }

    /// The report so far.
    #[must_use]
    pub const fn report(&self) -> &PipelineReport {
        &self.report
    }

    /// Consumes the pipeline, yielding the report.
    ///
    /// Refuses to yield a report that claims a mandatory stage did not run, because
    /// that would mean the caller is about to present an incomplete analysis as a
    /// complete one. A run that legitimately stops early returns an error instead,
    /// with the stage that stopped it named.
    pub fn finish(self) -> Result<PipelineReport> {
        let missing = self.report.missing_mandatory_stages();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|stage| stage.as_str()).collect();
            return Err(EngineError::Internal(format!(
                "the pipeline finished without running mandatory stage(s): {}",
                names.join(", ")
            )));
        }
        Ok(self.report)
    }

    /// Consumes the pipeline, yielding the report without the completeness check.
    ///
    /// Used when a run is expected to stop early - `snapshot diff` does not inspect
    /// a contract, for example - so that the caller can state which stages are
    /// applicable rather than having the pipeline guess.
    #[must_use]
    pub fn finish_partial(self) -> PipelineReport {
        self.report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::EngineConfig;
    use crate::context::ExecutionContext;
    use crate::identity::LedgerSequence;
    use crate::observations::{Network, NetworkType};

    fn context() -> ExecutionContext {
        ExecutionContext::new(
            EngineConfig::default(),
            Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("valid"),
            LedgerSequence::new(1).expect("valid"),
            "2026-09-15T00:00:00Z",
        )
        .expect("valid")
    }

    #[test]
    fn the_stage_order_places_inspection_before_resolution_and_graph_before_impact() {
        let stages = Stage::all();
        let index = |stage: Stage| {
            stages
                .iter()
                .position(|candidate| *candidate == stage)
                .expect("every stage is listed")
        };
        // Ordering is normative: a dependency is established from something
        // observed, and impact traverses a graph that must already exist.
        assert!(index(Stage::InspectContract) < index(Stage::ResolveDependencies));
        assert!(index(Stage::ResolveDependencies) < index(Stage::BuildGraph));
        assert!(index(Stage::BuildGraph) < index(Stage::AnalyseImpact));
        assert!(index(Stage::CollectEvidence) < index(Stage::VerifyProvenance));
        assert!(index(Stage::LoadConfiguration) < index(Stage::LoadSpecification));
    }

    #[test]
    fn the_stage_list_is_in_the_order_the_enum_declares() {
        let mut sorted = Stage::all().to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            Stage::all(),
            "the declared order must be the enum order"
        );
    }

    #[test]
    fn every_stage_has_a_distinct_stable_name() {
        let mut names: Vec<&str> = Stage::all().iter().map(|stage| stage.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }

    #[test]
    fn the_mandatory_and_conditional_stages_partition_the_list() {
        for stage in Stage::all() {
            assert_ne!(
                stage.is_mandatory(),
                stage.is_conditional(),
                "{stage} must be exactly one of mandatory or conditional"
            );
        }
        let mandatory = Stage::all().iter().filter(|s| s.is_mandatory()).count();
        let conditional = Stage::all().iter().filter(|s| s.is_conditional()).count();
        assert_eq!(mandatory + conditional, Stage::all().len());
        // The snapshot, diff and export stages are conditional, because a run that
        // does not request them must not be forced to perform them.
        assert!(Stage::CaptureSnapshot.is_conditional());
        assert!(Stage::CompareSnapshots.is_conditional());
        assert!(Stage::Export.is_conditional());
        assert!(Stage::AnalyseImpact.is_conditional());
    }

    #[test]
    fn a_pipeline_records_what_ran_and_what_was_skipped() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        pipeline
            .run(Stage::LoadConfiguration, |_| Ok("loaded"))
            .expect("the stage succeeds");
        pipeline.skip(Stage::AnalyseImpact, "no change was supplied");
        let report = pipeline.finish_partial();

        assert!(
            report
                .outcome_of(Stage::LoadConfiguration)
                .expect("recorded")
                .ran()
        );
        let skipped = report.outcome_of(Stage::AnalyseImpact).expect("recorded");
        assert!(!skipped.ran());
        // The reason is recorded, because a consumer cannot distinguish an omitted
        // stage from one that produced nothing.
        assert_eq!(skipped.reason(), Some("no change was supplied"));
    }

    #[test]
    fn a_stage_that_fails_is_not_recorded_as_having_completed() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        let result: Result<()> = pipeline.run(Stage::InspectContract, |_| {
            Err(EngineError::ContractNotFound {
                contract_id: "C2IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26XB".to_owned(),
                network: "testnet".to_owned(),
                ledger: 1,
            })
        });
        result.expect_err("the stage fails");

        let outcome = pipeline
            .report()
            .outcome_of(Stage::InspectContract)
            .expect("recorded");
        // Recording a failure as a completion is how a report comes to claim an
        // analysis that never happened.
        assert!(!outcome.ran());
        assert!(
            outcome
                .reason()
                .expect("a reason")
                .contains("was not found"),
            "the reason must carry the underlying failure: {:?}",
            outcome.reason()
        );
    }

    #[test]
    fn a_cancelled_run_stops_at_a_stage_boundary() {
        let context = context();
        context.cancellation().cancel();
        let mut pipeline = Pipeline::new(&context);
        let error = pipeline
            .run(Stage::InspectContract, |_| Ok("should not run"))
            .expect_err("a cancelled run does not perform work");
        assert!(error.to_string().contains("cancelled"));
        // The stage was not attempted at all, so there is no partially applied
        // result to reason about.
        assert!(
            pipeline
                .report()
                .outcome_of(Stage::InspectContract)
                .is_none()
        );
    }

    #[test]
    fn truncation_is_distinct_from_completion() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        pipeline.truncate(
            Stage::ResolveDependencies,
            "the endpoint applied a rate limit",
            "resolved 12 of an unknown number of edges",
        );
        let report = pipeline.finish_partial();
        assert!(report.is_truncated());
        assert_eq!(report.truncated_stages(), vec![Stage::ResolveDependencies]);
        let outcome = report
            .outcome_of(Stage::ResolveDependencies)
            .expect("recorded");
        assert!(outcome.ran());
        assert!(outcome.is_truncated());
        assert!(outcome.reason().expect("a reason").contains("rate limit"));
    }

    #[test]
    fn a_repeated_stage_replaces_its_record_rather_than_duplicating_it() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        pipeline.complete(Stage::InspectContract, "first attempt");
        pipeline.complete(Stage::InspectContract, "second attempt");
        let report = pipeline.finish_partial();
        assert_eq!(report.stages().len(), 1);
        assert_eq!(
            report
                .outcome_of(Stage::InspectContract)
                .and_then(StageOutcome::summary),
            Some("second attempt")
        );
    }

    #[test]
    fn a_report_refuses_to_complete_when_a_mandatory_stage_did_not_run() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        pipeline.complete(Stage::LoadConfiguration, "loaded");
        // Every other mandatory stage is missing, so the report cannot be presented
        // as an analysis.
        let error = pipeline.finish().expect_err("mandatory stages are missing");
        assert!(error.to_string().contains("mandatory stage"));
    }

    #[test]
    fn a_pipeline_that_ran_every_mandatory_stage_completes() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        for stage in Stage::all().iter().copied().filter(|s| s.is_mandatory()) {
            pipeline.complete(stage, "done");
        }
        let report = pipeline.finish().expect("every mandatory stage ran");
        assert!(report.missing_mandatory_stages().is_empty());
        assert!(!report.is_truncated());
    }

    #[test]
    fn the_report_summary_counts_completed_skipped_and_truncated_stages() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        pipeline.complete(Stage::LoadConfiguration, "one");
        pipeline.complete(Stage::LoadSpecification, "two");
        pipeline.skip(Stage::AnalyseImpact, "not requested");
        pipeline.truncate(Stage::ResolveDependencies, "rate limited", "12 edges");
        let summary = pipeline.finish_partial().summary();
        assert_eq!(summary, "3 stage(s) completed, 1 skipped, 1 truncated");
    }

    #[test]
    fn a_completed_stage_records_how_long_it_took_when_measured() {
        let context = context();
        let mut pipeline = Pipeline::new(&context);
        pipeline
            .run(Stage::LoadConfiguration, |_| Ok(()))
            .expect("succeeds");
        let outcome = pipeline
            .report()
            .outcome_of(Stage::LoadConfiguration)
            .expect("recorded");
        match outcome {
            StageOutcome::Completed { elapsed, .. } => {
                assert!(elapsed.is_some(), "a measured stage records its duration");
            },
            other => panic!("expected a completion, got {other:?}"),
        }
    }

    #[test]
    fn an_outcome_reports_the_field_that_matches_its_variant() {
        assert!(StageOutcome::completed("x").summary().is_some());
        assert!(StageOutcome::completed("x").reason().is_none());
        assert!(StageOutcome::skipped("because").reason().is_some());
        assert!(StageOutcome::skipped("because").summary().is_none());
        assert!(StageOutcome::truncated("because", "x").reason().is_some());
        assert!(StageOutcome::truncated("because", "x").summary().is_some());
    }
}
