//! Turning observations into candidates, and saying what produced nothing.
//!
//! # Why this module reports what it skipped
//!
//! The distinction a dependency report must never lose is between *no dependency
//! found* and *nothing observed*. A module that silently drops the observations it
//! cannot use produces the same output as one that looked and found nothing, and the
//! reader cannot tell which happened. [`DetectionReport`] therefore carries the
//! observations it considered, the candidates it built, and the ones it set aside
//! with the reason - so "no cross-contract calls were observed" and "the calls that
//! were observed were made by transactions rather than by contracts" are different
//! statements.
//!
//! # What an observation has to contain to become a candidate
//!
//! A cross-contract invocation must have a **calling contract**. A top-level call has
//! no caller at all - the transaction's source account is not a contract - and
//! recording the source account as though it were one would attribute a call to an
//! account. [`SkipReason::TopLevelInvocation`] names that case rather than inventing a
//! subject for it.
//!
//! An invocation of the subject by itself is likewise not a dependency: a self-loop
//! is a graph defect, and [`SkipReason::SelfInvocation`] says so.
//!
//! An invocation whose transaction **failed** is *not* skipped. The call was
//! attempted and the attempt is a real observation; what it cannot do is establish
//! runtime use. The candidate carries the recorded outcome and
//! [`crate::classifier::classify`] refuses the `RUNTIME` class, so the refusal
//! surfaces as an unestablished dependency with its reason rather than as silence.

use amasario_contract::ContractInvocation;
use amasario_core::{
    Basis, EntityKind, EntityRef, EvidenceType, ObservationBoundary, Result, TransactionHash,
};
use serde::{Deserialize, Serialize};

use crate::classifier::{Candidate, EvidenceRef};

/// A dependency named by a project's own declaration rather than observed on a
/// network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredDependency {
    /// The package's name.
    pub name: String,
    /// The version as the declaration states it, where it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether a lockfile resolved the declaration to a specific version.
    ///
    /// The distinction sets the basis: a resolved lockfile pins an artifact, and a
    /// bare declaration does not. Reporting both as `DECLARED_MANIFEST` would
    /// understate a resolved lockfile, and reporting both as `RESOLVED_LOCKFILE`
    /// would overstate a declaration.
    pub resolved: bool,
    /// The identifier of the evidence record the declaration came from.
    pub evidence_id: String,
}

impl DeclaredDependency {
    /// The name as a reference to a package entity.
    fn as_package(&self) -> Result<EntityRef> {
        let name = match &self.version {
            Some(version) => format!("{} {version}", self.name),
            None => self.name.clone(),
        };
        EntityRef::new(EntityKind::Package, name)
    }

    /// The basis the declaration supports.
    #[must_use]
    pub const fn basis(&self) -> Basis {
        if self.resolved {
            Basis::ResolvedLockfile
        } else {
            Basis::DeclaredManifest
        }
    }
}

/// Why an observation produced no candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum SkipReason {
    /// A call made directly by a transaction, with no calling contract.
    TopLevelInvocation,
    /// A call whose caller is another contract rather than the subject.
    ///
    /// The invocation happened, and the callee is real; it simply says nothing about
    /// what *this* subject requires. Distinguishing it from a self-call matters
    /// because the two are different facts about the graph.
    AttributedToAnotherContract,
    /// A call in which the subject was entered rather than the one entering.
    SelfInvocation,
}

impl SkipReason {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TopLevelInvocation => "TOP_LEVEL_INVOCATION",
            Self::AttributedToAnotherContract => "ATTRIBUTED_TO_ANOTHER_CONTRACT",
            Self::SelfInvocation => "SELF_INVOCATION",
        }
    }

    /// Every reason.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::TopLevelInvocation,
            Self::AttributedToAnotherContract,
            Self::SelfInvocation,
        ]
    }
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An observation that produced no candidate, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkippedObservation {
    /// Why it was set aside.
    pub reason: SkipReason,
    /// What was observed, in a reader's words.
    pub detail: String,
    /// The transaction it came from, where one was named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,
}

/// What a detection pass considered and what it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionReport {
    /// The entity the pass was about.
    pub subject: EntityRef,
    /// The boundary every candidate was observed at.
    pub boundary: ObservationBoundary,
    /// How many observations were examined.
    pub considered: usize,
    /// The candidates that were built.
    pub candidates: Vec<Candidate>,
    /// The observations that produced no candidate, with their reasons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedObservation>,
}

impl DetectionReport {
    /// Whether any candidate was produced.
    #[must_use]
    pub const fn has_candidates(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// Whether every observation was set aside.
    ///
    /// The state that must not be reported as "no dependencies exist": the pass has
    /// nothing to say either way.
    #[must_use]
    pub const fn observed_nothing_usable(&self) -> bool {
        self.candidates.is_empty() && self.considered > 0
    }

    /// How many observations were set aside for a reason.
    #[must_use]
    pub fn skipped_for(&self, reason: SkipReason) -> usize {
        self.skipped
            .iter()
            .filter(|skipped| skipped.reason == reason)
            .count()
    }
}

/// Builds candidates from observed cross-contract invocations.
///
/// # Errors
///
/// Returns a dependency error when a candidate cannot be constructed, which happens
/// when a citation is empty. Invocations that cannot express a dependency are set
/// aside with a reason rather than dropped.
pub fn detect_from_invocations(
    subject: &EntityRef,
    boundary: &ObservationBoundary,
    invocations: &[ContractInvocation],
) -> Result<DetectionReport> {
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for invocation in invocations {
        let transaction = invocation.transaction.as_str().to_owned();
        let Some(caller) = invocation.caller.as_deref() else {
            skipped.push(SkippedObservation {
                reason: SkipReason::TopLevelInvocation,
                detail: format!(
                    "{} was called by a transaction rather than by a contract, so no contract \
                     depends on it here",
                    invocation.callee
                ),
                transaction: Some(transaction),
            });
            continue;
        };
        // The invoking contract is the subject. A call made by a different contract is
        // real but belongs to that contract's dependency set, and a call in which the
        // subject was itself entered is not the subject depending on anything.
        if caller != subject.id {
            skipped.push(SkippedObservation {
                reason: SkipReason::AttributedToAnotherContract,
                detail: format!(
                    "{} was called by {caller} rather than by {}, so the call says nothing about \
                     what {} requires",
                    invocation.callee, subject.id, subject.id
                ),
                transaction: Some(transaction),
            });
            continue;
        }
        if invocation.callee == subject.id {
            skipped.push(SkippedObservation {
                reason: SkipReason::SelfInvocation,
                detail: format!(
                    "{} called itself, which is a graph defect rather than a dependency",
                    subject.id
                ),
                transaction: Some(transaction),
            });
            continue;
        }

        let object = EntityRef::new(EntityKind::Contract, invocation.callee.as_str())?;
        let mut evidence = vec![EvidenceRef::new(
            EvidenceType::Transaction,
            transaction.clone(),
        )?];
        if let Some(event_id) = invocation.event_id.as_deref() {
            evidence.push(EvidenceRef::new(EvidenceType::Event, event_id)?);
        }
        let mut detail = match invocation.function.as_deref() {
            Some(function) => format!("{caller} called {function} on {}", invocation.callee),
            None => format!("{caller} called {}", invocation.callee),
        };
        if let Some(ledger) = invocation.ledger {
            detail = format!("{detail}, in ledger {ledger}");
        }
        candidates.push(
            Candidate::new(
                subject.clone(),
                object,
                amasario_core::Relationship::Invocates,
                invocation.basis,
                evidence,
            )?
            .observed_at(boundary.clone())
            .with_outcome(invocation.successful)
            .explained_by(detail),
        );
    }

    sort_candidates(&mut candidates);
    skipped.sort_by(|left, right| left.detail.cmp(&right.detail));
    Ok(DetectionReport {
        subject: subject.clone(),
        boundary: boundary.clone(),
        considered: invocations.len(),
        candidates,
        skipped,
    })
}

/// Builds candidates from dependencies a project declares about itself.
///
/// # Errors
///
/// Returns a dependency error when a candidate cannot be constructed.
///
/// A declaration is evidence about the subject's own inputs, which is why it can
/// establish a `PACKAGE` dependency and cannot establish a `RUNTIME` one.
pub fn detect_from_declared(
    subject: &EntityRef,
    boundary: &ObservationBoundary,
    declared: &[DeclaredDependency],
) -> Result<DetectionReport> {
    let mut candidates = Vec::new();
    for dependency in declared {
        let kind = if dependency.resolved {
            // A resolved lockfile names an artifact, so the evidence is about a build
            // rather than about a source.
            EvidenceType::Build
        } else {
            EvidenceType::Source
        };
        candidates.push(
            Candidate::new(
                subject.clone(),
                dependency.as_package()?,
                amasario_core::Relationship::DependsOn,
                dependency.basis(),
                vec![EvidenceRef::new(kind, dependency.evidence_id.clone())?],
            )?
            .observed_at(boundary.clone())
            .explained_by(match &dependency.version {
                Some(version) => format!("the subject declares {} {version}", dependency.name),
                None => format!("the subject declares {}", dependency.name),
            }),
        );
    }
    sort_candidates(&mut candidates);
    Ok(DetectionReport {
        subject: subject.clone(),
        boundary: boundary.clone(),
        considered: declared.len(),
        candidates,
        skipped: Vec::new(),
    })
}

/// Orders candidates so that two passes over the same observations agree.
fn sort_candidates(candidates: &mut [Candidate]) {
    candidates.sort_by(|left, right| {
        left.object
            .kind
            .as_str()
            .cmp(right.object.kind.as_str())
            .then_with(|| left.object.id.cmp(&right.object.id))
            .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
            .then_with(|| left.basis.as_str().cmp(right.basis.as_str()))
    });
}

/// The transaction a candidate was observed in, where one was cited.
#[must_use]
pub fn transaction_of(candidate: &Candidate) -> Option<&str> {
    candidate
        .evidence
        .iter()
        .find(|evidence| evidence.kind == EvidenceType::Transaction)
        .map(|evidence| evidence.id.as_str())
}

/// Whether a string is a well-formed transaction hash, so that a caller can check a
/// citation before recording it.
#[must_use]
pub fn is_transaction_hash(value: &str) -> bool {
    TransactionHash::new(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{LedgerSequence, Network, NetworkType};

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(2_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn subject() -> EntityRef {
        EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference")
    }

    fn invocation(caller: Option<&str>, callee: &str, transaction: &str) -> ContractInvocation {
        ContractInvocation {
            callee: callee.to_owned(),
            caller: caller.map(ToOwned::to_owned),
            function: Some("transfer".to_owned()),
            transaction: TransactionHash::new(transaction).expect("a transaction hash"),
            ledger: Some(LedgerSequence::new(2_000).expect("a ledger")),
            operation_index: Some(0),
            basis: Basis::ObservedInvocation,
            event_id: None,
            successful: Some(true),
        }
    }

    #[test]
    fn a_cross_contract_call_by_the_subject_becomes_a_candidate() {
        let report = detect_from_invocations(
            &subject(),
            &boundary(),
            &[invocation(Some("C-subject"), "C-callee", &"a".repeat(64))],
        )
        .expect("detection succeeds");
        assert_eq!(report.considered, 1);
        assert_eq!(report.candidates.len(), 1);
        assert!(report.skipped.is_empty());
        let candidate = &report.candidates[0];
        assert_eq!(candidate.object.id, "C-callee");
        assert_eq!(candidate.basis, Basis::ObservedInvocation);
        assert_eq!(candidate.successful, Some(true));
        assert_eq!(
            candidate.network().map(|network| network.id.as_str()),
            Some("testnet")
        );
        assert_eq!(transaction_of(candidate), Some("a".repeat(64).as_str()));
    }

    #[test]
    fn a_top_level_call_is_set_aside_rather_than_attributed_to_an_account() {
        let report = detect_from_invocations(
            &subject(),
            &boundary(),
            &[invocation(None, "C-callee", &"b".repeat(64))],
        )
        .expect("detection succeeds");
        assert!(report.candidates.is_empty());
        assert!(report.observed_nothing_usable());
        assert_eq!(report.skipped_for(SkipReason::TopLevelInvocation), 1);
        assert!(!report.has_candidates());
    }

    #[test]
    fn a_self_call_is_a_graph_defect_rather_than_a_dependency() {
        let report = detect_from_invocations(
            &subject(),
            &boundary(),
            &[invocation(Some("C-subject"), "C-subject", &"c".repeat(64))],
        )
        .expect("detection succeeds");
        assert!(report.candidates.is_empty());
        assert_eq!(report.skipped_for(SkipReason::SelfInvocation), 1);
    }

    #[test]
    fn a_call_by_another_contract_says_nothing_about_this_subject() {
        let report = detect_from_invocations(
            &subject(),
            &boundary(),
            &[invocation(Some("C-elsewhere"), "C-callee", &"d".repeat(64))],
        )
        .expect("detection succeeds");
        assert!(
            report.candidates.is_empty(),
            "another contract's call must not become this subject's dependency"
        );
        assert_eq!(report.considered, 1);
        assert_eq!(
            report.skipped_for(SkipReason::AttributedToAnotherContract),
            1,
            "the reason distinguishes another contract's call from a self-call"
        );
    }

    #[test]
    fn a_failed_transaction_still_produces_a_candidate_so_the_refusal_can_be_reported() {
        // The call was attempted and the attempt is a real observation. What it cannot
        // do is establish runtime use, and the classifier is where that is decided -
        // dropping it here would turn a refusal into silence.
        let mut failed = invocation(Some("C-subject"), "C-callee", &"e".repeat(64));
        failed.successful = Some(false);
        let report = detect_from_invocations(&subject(), &boundary(), &[failed])
            .expect("detection succeeds");
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].successful, Some(false));
    }

    #[test]
    fn a_diagnostic_event_invocation_cites_both_evidence_kinds() {
        let mut from_event = invocation(Some("C-subject"), "C-callee", &"f".repeat(64));
        from_event.basis = Basis::ObservedEvent;
        from_event.event_id = Some("0000001234-0".to_owned());
        let report = detect_from_invocations(&subject(), &boundary(), &[from_event])
            .expect("detection succeeds");
        let kinds = report.candidates[0].cited_kinds();
        assert!(kinds.contains(&EvidenceType::Transaction));
        assert!(kinds.contains(&EvidenceType::Event));
    }

    #[test]
    fn declared_dependencies_distinguish_a_lockfile_from_a_manifest() {
        let declared = vec![
            DeclaredDependency {
                name: "soroban-sdk".to_owned(),
                version: Some("22.0.0".to_owned()),
                resolved: true,
                evidence_id: "lock-1".to_owned(),
            },
            DeclaredDependency {
                name: "some-other-crate".to_owned(),
                version: None,
                resolved: false,
                evidence_id: "manifest-1".to_owned(),
            },
        ];
        let report =
            detect_from_declared(&subject(), &boundary(), &declared).expect("detection succeeds");
        assert_eq!(report.candidates.len(), 2);
        assert!(report.skipped.is_empty());
        let resolved = report
            .candidates
            .iter()
            .find(|candidate| candidate.object.id.starts_with("soroban-sdk"))
            .expect("the resolved declaration");
        let manifest = report
            .candidates
            .iter()
            .find(|candidate| candidate.object.id.starts_with("some-other-crate"))
            .expect("the bare declaration");
        assert_eq!(resolved.basis, Basis::ResolvedLockfile);
        assert_eq!(resolved.object.id, "soroban-sdk 22.0.0");
        assert_eq!(manifest.basis, Basis::DeclaredManifest);
        // The order is by target, so the two are deterministically placed rather than
        // left in declaration order.
        assert_eq!(report.candidates[0], manifest.clone());
    }

    #[test]
    fn detection_is_deterministic_and_ordered_by_target() {
        let invocations = vec![
            invocation(Some("C-subject"), "C-zeta", &"1".repeat(64)),
            invocation(Some("C-subject"), "C-alpha", &"2".repeat(64)),
        ];
        let first = detect_from_invocations(&subject(), &boundary(), &invocations)
            .expect("detection succeeds");
        let second = detect_from_invocations(&subject(), &boundary(), &invocations)
            .expect("detection succeeds");
        assert_eq!(first, second);
        assert_eq!(first.candidates[0].object.id, "C-alpha");
        assert_eq!(first.candidates[1].object.id, "C-zeta");
    }

    #[test]
    fn an_empty_pass_is_not_a_pass_that_found_nothing() {
        let report =
            detect_from_invocations(&subject(), &boundary(), &[]).expect("detection succeeds");
        assert_eq!(report.considered, 0);
        assert!(!report.observed_nothing_usable());
    }

    #[test]
    fn a_transaction_hash_citation_is_checkable() {
        assert!(is_transaction_hash(&"a".repeat(64)));
        assert!(!is_transaction_hash("not a hash"));
    }

    #[test]
    fn every_skip_reason_has_a_stable_name() {
        for reason in SkipReason::all() {
            assert!(SkipReason::all().contains(reason));
            assert!(!reason.as_str().is_empty());
            assert_eq!(reason.to_string(), reason.as_str());
        }
        assert_eq!(SkipReason::all().len(), 3);
    }
}
