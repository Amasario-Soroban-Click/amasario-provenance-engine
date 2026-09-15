//! Verification: what the evidence in a chain actually says.
//!
//! # The precedence rule
//!
//! `CONFLICTING` wins over everything. One contradicted link is enough to make the
//! whole chain `CONFLICTING`, regardless of how many other links are verified,
//! because a chain is a conjunction: it asserts that every stage holds, and one stage
//! that cannot hold makes the conjunction false. Reporting `PARTIALLY_VERIFIED` for a
//! chain with one refuted link would be the most damaging output this system can
//! produce, and the precedence rule exists so that no combination of confident links
//! can out-vote a contradiction.
//!
//! # What is not a contradiction
//!
//! Failure to check is not failure. A link with no evidence is `UNVERIFIED`; a link
//! whose evidence cannot be compared - two digests under different algorithms - is
//! also inconclusive. Neither refutes the claim, and neither may be reported as
//! though it did.
//!
//! # Completeness is separate from verification
//!
//! A chain whose stages are all unverified but all `VERIFIED` in coverage is not the
//! same as a chain with a stage nobody established. [`VerificationOutcome`] reports
//! both: the status says what the evidence supports, and `missing_links` says which
//! stages have nothing behind them at all. A caller deciding whether to trust an
//! analysis needs both, and a single status cannot carry them.

use std::fmt;

use amasario_core::{ConfidenceLevel, VerificationStatus};
use serde::{Deserialize, Serialize};

use crate::matching::{ChainLinkKind, MatchOutcome, ProvenanceChain};

/// What was found out about one stage of a chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkAssessment {
    /// Which stage was assessed.
    pub kind: ChainLinkKind,
    /// What the evidence says about it.
    pub status: VerificationStatus,
    /// Why, in terms of the evidence. Always present: an assessment a reader cannot
    /// argue with is a verdict rather than a finding.
    pub reason: String,
}

/// The result of verifying a chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationOutcome {
    /// The combined status.
    pub status: VerificationStatus,
    /// What was found about each link the chain holds.
    pub assessments: Vec<LinkAssessment>,
    /// The stages the chain has no link for.
    pub missing_links: Vec<ChainLinkKind>,
    /// The links whose evidence contradicts their claim.
    pub contradictions: Vec<ChainLinkKind>,
    /// The weakest confidence among the links that were assessed.
    ///
    /// Carried alongside the status because the two answer different questions:
    /// `PARTIALLY_VERIFIED` says some components checked out, and the confidence says
    /// how well supported the best of them were.
    pub weakest_confidence: Option<ConfidenceLevel>,
}

impl VerificationOutcome {
    /// Whether the chain may be reported as verified as a whole.
    ///
    /// Requires every applicable stage present, every link affirmed, and no
    /// contradiction anywhere. Stated as one method so that a caller does not
    /// re-derive the condition and get it subtly wrong - which is how a bounded search
    /// comes to be reported as a complete one.
    #[must_use]
    pub const fn is_verified(&self) -> bool {
        matches!(self.status, VerificationStatus::Verified) && self.missing_links.is_empty()
    }

    /// Whether the outcome means the chain cannot hold together.
    #[must_use]
    pub const fn is_contradicted(&self) -> bool {
        matches!(self.status, VerificationStatus::Conflicting)
    }

    /// The one-line explanation a report leads with.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.is_contradicted() {
            return format!(
                "{} link(s) are contradicted by their evidence, so the chain cannot hold together",
                self.contradictions.len()
            );
        }
        if !self.missing_links.is_empty() {
            return format!(
                "{} of {} stages have no evidence behind them",
                self.missing_links.len(),
                ChainLinkKind::all().len()
            );
        }
        format!(
            "all {} stages were assessed as {}",
            self.assessments.len(),
            self.status
        )
    }
}

impl fmt::Display for VerificationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

/// Verifies a chain.
///
/// The combination follows the specification's precedence: a contradiction wins;
/// otherwise an affirmation beats an inconclusive result; among inconclusive results
/// `UNVERIFIED` beats `UNKNOWN`, because knowing a claim is unchecked says more than
/// being unable to evaluate it.
///
/// A chain with no links at all is `UNKNOWN`: the engine holds no claim to verify, and
/// reporting `UNVERIFIED` would suggest a claim was made and not checked.
#[must_use]
pub fn verify_chain(chain: &ProvenanceChain) -> VerificationOutcome {
    if chain.links.is_empty() {
        return VerificationOutcome {
            status: VerificationStatus::Unknown,
            assessments: Vec::new(),
            missing_links: chain.missing_links(),
            contradictions: Vec::new(),
            weakest_confidence: None,
        };
    }

    // Seeded from the first link rather than from `Unknown`. `combine` has no
    // identity element - `Unknown` combined with `Verified` is `PARTIALLY_VERIFIED`,
    // which is the right answer for two *components* of one claim and the wrong
    // answer for a fold's starting point. Seeding with `Unknown` would report a
    // fully verified chain as partially verified.
    let mut combined: Option<VerificationStatus> = None;
    let mut assessments = Vec::with_capacity(chain.links.len());
    let mut contradictions = Vec::new();

    for link in &chain.links {
        // Confidence that carries contradicting evidence is treated as a refutation
        // even when the link's own status does not say so. The two are kept separate
        // in the model because they mean different things, but a link whose
        // counter-evidence was recorded cannot be reported as affirmed whatever its
        // status field says.
        let effective = if link.confidence.is_contradicted() {
            VerificationStatus::Conflicting
        } else {
            link.verification
        };

        if effective.is_refutation() {
            contradictions.push(link.kind);
        }

        assessments.push(LinkAssessment {
            kind: link.kind,
            status: effective,
            reason: reason_for(link),
        });
        combined = Some(match combined {
            Some(combined) => combined.combine(effective),
            None => effective,
        });
    }

    VerificationOutcome {
        status: combined.unwrap_or(VerificationStatus::Unknown),
        assessments,
        missing_links: chain.missing_links(),
        contradictions,
        weakest_confidence: chain.weakest_confidence(),
    }
}

/// Explains what a link's evidence says about it.
///
/// Built from the link rather than stored, so that the explanation cannot drift from
/// the fields it explains.
fn reason_for(link: &crate::matching::ChainLink) -> String {
    let establishes = link.kind.establishes();
    if link.confidence.is_contradicted() {
        return format!(
            "{establishes}: contradicted by {} counter-evidence record(s), which wins over the \
             claim's own status",
            link.confidence.contradicting_evidence.len()
        );
    }
    match link.verification {
        VerificationStatus::Conflicting => {
            format!("{establishes}: the evidence contradicts the claim")
        },
        VerificationStatus::Verified => format!(
            "{establishes}: checked against {} evidence record(s) on a {} basis",
            link.confidence.evidence.len(),
            link.basis
        ),
        VerificationStatus::PartiallyVerified => format!(
            "{establishes}: some components checked successfully on a {} basis; none contradicted",
            link.basis
        ),
        VerificationStatus::Unverified => format!(
            "{establishes}: not checked. The record rests on the {} basis at {} confidence, which \
             is a claim about how the fact was obtained rather than a check that it holds",
            link.basis, link.confidence.level
        ),
        VerificationStatus::Unknown => format!(
            "{establishes}: the claim could not be evaluated at all, so nothing follows about it"
        ),
        // `VerificationStatus` is non-exhaustive, so a status added by a later
        // specification version reaches this arm. It is deliberately not mapped onto
        // a status the engine does define: that would be the engine inventing a
        // meaning for a term it does not know, and mapping an unrecognised status to
        // `Verified` would be the most damaging guess available. The link is instead
        // reported as uninterpretable, which is the truthful statement.
        _ => format!(
            "{establishes}: the recorded status {status} is not defined at this specification \
             version and cannot be interpreted, so nothing follows about it",
            status = link.verification
        ),
    }
}

/// The status a comparison outcome supports when it is applied to one claim.
///
/// A `Mismatch` is `CONFLICTING`; an `Incomparable` is `UNVERIFIED`, never
/// `CONFLICTING`. This is the single place where a comparison becomes a status, so
/// the rule cannot be applied differently in two places.
#[must_use]
pub const fn status_for_match(outcome: MatchOutcome) -> VerificationStatus {
    match outcome {
        MatchOutcome::Match => VerificationStatus::Verified,
        MatchOutcome::Mismatch => VerificationStatus::Conflicting,
        MatchOutcome::Incomparable => VerificationStatus::Unverified,
    }
}

/// Verifies a digest comparison and, on a mismatch, returns the contradiction it
/// establishes.
///
/// # Errors
///
/// Returns a provenance error when the comparison is a mismatch, so that a caller
/// which has nothing useful to do with a contradiction cannot continue as though the
/// check passed. A caller that *does* want to record the contradiction rather than
/// abort should use [`status_for_match`] and read the two values itself.
pub fn require_digest_match(
    claimed: &amasario_core::Digest,
    computed: &amasario_core::Digest,
    subject: &str,
) -> amasario_core::Result<()> {
    match crate::matching::match_digests(claimed, computed) {
        MatchOutcome::Match => Ok(()),
        MatchOutcome::Mismatch => Err(crate::errors::ProvenanceFailure::DigestContradiction {
            claimed: claimed.value().to_owned(),
            computed: computed.value().to_owned(),
            subject: subject.to_owned(),
        }
        .into_error()),
        // An inability to compare is not a refutation and must not be reported as
        // one, so it is not an error: the caller has to decide what to do about a
        // comparison it cannot make.
        MatchOutcome::Incomparable => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::ChainLink;
    use amasario_core::{Basis, Confidence, Digest, EntityKind, EntityRef};

    fn digest(seed: u8) -> Digest {
        Digest::sha256_of(&[seed])
    }

    fn confidence(level: ConfidenceLevel, contradicting: Vec<String>) -> Confidence {
        Confidence::new(level, vec!["e".to_owned()], contradicting).expect("a confidence")
    }

    fn contract() -> EntityRef {
        EntityRef::new(EntityKind::Contract, "C-x").expect("a reference")
    }

    fn source() -> EntityRef {
        EntityRef::new(EntityKind::Source, "source-1").expect("a reference")
    }

    fn push(
        chain: &mut ProvenanceChain,
        kind: ChainLinkKind,
        status: VerificationStatus,
        confidence: Confidence,
    ) {
        let object = kind
            .object_kind()
            .map(|entity| EntityRef::new(entity, "o").expect("a reference"));
        chain
            .push(
                ChainLink::new(
                    kind,
                    EntityRef::new(kind.subject_kind(), "s").expect("r"),
                    object,
                    Basis::ObservedInvocation,
                    confidence,
                    status,
                )
                .expect("a permitted link"),
            )
            .expect("in order");
    }

    fn chain_with(status: VerificationStatus, contradicting: Vec<String>) -> ProvenanceChain {
        let mut chain = ProvenanceChain::new(contract()).expect("a contract");
        for kind in ChainLinkKind::all() {
            push(
                &mut chain,
                *kind,
                status,
                confidence(ConfidenceLevel::Verified, contradicting.clone()),
            );
        }
        chain
    }

    #[test]
    fn an_empty_chain_is_unknown_rather_than_unverified() {
        // The difference matters: UNVERIFIED would suggest a claim was made and not
        // checked, when in fact no claim was made.
        let chain = ProvenanceChain::new(contract()).expect("a contract");
        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::Unknown);
        assert!(!outcome.is_verified());
        assert!(!outcome.is_contradicted());
        assert!(outcome.assessments.is_empty());
        assert_eq!(outcome.missing_links.len(), ChainLinkKind::all().len());
        assert!(outcome.summary().contains("have no evidence behind them"));
    }

    #[test]
    fn a_fully_verified_chain_is_verified_and_complete() {
        let outcome = verify_chain(&chain_with(VerificationStatus::Verified, Vec::new()));
        assert_eq!(outcome.status, VerificationStatus::Verified);
        assert!(outcome.is_verified());
        assert!(outcome.missing_links.is_empty());
        assert!(outcome.contradictions.is_empty());
        assert_eq!(outcome.weakest_confidence, Some(ConfidenceLevel::Verified));
        assert_eq!(outcome.assessments.len(), ChainLinkKind::all().len());
    }

    #[test]
    fn one_contradicted_link_makes_the_whole_chain_conflicting() {
        // The precedence rule, and the property the specification requires: no
        // combination of confident links may out-vote a contradiction.
        let mut chain = ProvenanceChain::new(contract()).expect("a contract");
        for kind in ChainLinkKind::all() {
            let status = if *kind == ChainLinkKind::ArtifactToWasm {
                VerificationStatus::Conflicting
            } else {
                VerificationStatus::Verified
            };
            push(
                &mut chain,
                *kind,
                status,
                confidence(ConfidenceLevel::Verified, Vec::new()),
            );
        }

        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::Conflicting);
        assert!(outcome.is_contradicted());
        assert!(!outcome.is_verified());
        assert_eq!(outcome.contradictions, vec![ChainLinkKind::ArtifactToWasm]);
        assert!(outcome.summary().contains("cannot hold together"));
    }

    #[test]
    fn contradicting_evidence_wins_over_a_verified_status() {
        // The two fields are separate because they mean different things, and a link
        // whose counter-evidence was recorded cannot be reported as affirmed whatever
        // its status field says.
        let chain = chain_with(
            VerificationStatus::Verified,
            vec!["counter-evidence".to_owned()],
        );
        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::Conflicting);
        assert_eq!(outcome.contradictions.len(), ChainLinkKind::all().len());
        assert!(
            outcome.assessments[0].reason.contains("counter-evidence"),
            "the reason must name what contradicted it: {}",
            outcome.assessments[0].reason
        );
    }

    #[test]
    fn an_unverified_link_leaves_the_chain_partially_verified() {
        // Failure to check is not failure.
        let mut chain = ProvenanceChain::new(contract()).expect("a contract");
        push(
            &mut chain,
            ChainLinkKind::SourceResolved,
            VerificationStatus::Verified,
            confidence(ConfidenceLevel::Verified, Vec::new()),
        );
        push(
            &mut chain,
            ChainLinkKind::SourceToBuild,
            VerificationStatus::Unverified,
            confidence(ConfidenceLevel::MediumConfidence, Vec::new()),
        );

        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::PartiallyVerified);
        assert!(!outcome.is_contradicted());
        assert!(
            !outcome.is_verified(),
            "a gap means the chain is not verified"
        );
        assert!(outcome.assessments[1].reason.contains("not checked"));
    }

    #[test]
    fn a_chain_whose_stages_are_merely_unknown_is_unknown() {
        let mut chain = ProvenanceChain::new(contract()).expect("a contract");
        push(
            &mut chain,
            ChainLinkKind::SourceResolved,
            VerificationStatus::Unknown,
            confidence(ConfidenceLevel::Unknown, Vec::new()),
        );
        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::Unknown);
        assert!(
            outcome.assessments[0]
                .reason
                .contains("could not be evaluated")
        );
    }

    #[test]
    fn a_comparison_becomes_a_status_in_exactly_one_place() {
        assert_eq!(
            status_for_match(MatchOutcome::Match),
            VerificationStatus::Verified
        );
        assert_eq!(
            status_for_match(MatchOutcome::Mismatch),
            VerificationStatus::Conflicting
        );
        assert_eq!(
            status_for_match(MatchOutcome::Incomparable),
            VerificationStatus::Unverified,
            "an inability to compare must never become a contradiction"
        );
    }

    #[test]
    fn a_digest_mismatch_is_refused_and_an_incomparable_pair_is_not() {
        require_digest_match(&digest(1), &digest(1), "the module").expect("the same bytes");

        let error = require_digest_match(&digest(1), &digest(2), "the module")
            .expect_err("different content cannot be the same content");
        let message = error.to_string();
        assert!(message.contains("the module"));
        assert!(message.contains("cannot both be correct"));

        // An inability to compare is not a refutation, so it is not an error: the
        // caller has to decide what to do about a comparison it cannot make.
        let sha512 = Digest::new(amasario_core::DigestAlgorithm::Sha512, &"a".repeat(128))
            .expect("a valid digest");
        require_digest_match(&digest(1), &sha512, "the module")
            .expect("an incomparable pair does not refute the claim");
    }

    #[test]
    fn a_verified_chain_with_a_missing_stage_is_not_reported_as_verified() {
        // The distinction a bounded search must not lose: the status says what the
        // evidence supports, and `missing_links` says which stages have nothing.
        let mut chain = ProvenanceChain::new(contract()).expect("a contract");
        push(
            &mut chain,
            ChainLinkKind::SourceResolved,
            VerificationStatus::Verified,
            confidence(ConfidenceLevel::Verified, Vec::new()),
        );

        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::Verified);
        assert!(
            !outcome.is_verified(),
            "every link present was verified, but stages are missing"
        );
        assert!(outcome.summary().contains("no evidence behind them"));
    }

    #[test]
    fn a_source_revision_that_rebuilds_differently_cannot_be_reported_as_verified() {
        // The specification's named example: a claimed source revision that cannot be
        // matched to the deployed WASM must be CONFLICTING rather than VERIFIED.
        let recorded = digest(7);
        let rebuilt = digest(8);
        assert_eq!(
            crate::matching::match_rebuilt_module(&recorded, &rebuilt),
            MatchOutcome::Mismatch
        );

        let mut chain = ProvenanceChain::new(contract()).expect("a contract");
        for kind in ChainLinkKind::all() {
            let status = if *kind == ChainLinkKind::SourceResolved {
                status_for_match(MatchOutcome::Mismatch)
            } else {
                VerificationStatus::Verified
            };
            push(
                &mut chain,
                *kind,
                status,
                confidence(ConfidenceLevel::Verified, Vec::new()),
            );
        }

        let outcome = verify_chain(&chain);
        assert_eq!(outcome.status, VerificationStatus::Conflicting);
        assert!(outcome.is_contradicted());
        assert!(!outcome.is_verified());
        assert_eq!(outcome.contradictions, vec![ChainLinkKind::SourceResolved]);
        assert!(chain.link(ChainLinkKind::SourceResolved).is_some());
        assert_eq!(source().kind, EntityKind::Source);
    }

    #[test]
    fn the_outcome_summarises_itself_for_each_disposition() {
        let verified = verify_chain(&chain_with(VerificationStatus::Verified, Vec::new()));
        assert!(verified.summary().contains("were assessed as VERIFIED"));

        let contradicted = verify_chain(&chain_with(VerificationStatus::Conflicting, Vec::new()));
        assert!(contradicted.summary().contains("cannot hold together"));
    }

    #[test]
    fn verification_is_deterministic() {
        let chain = chain_with(VerificationStatus::Verified, Vec::new());
        let first = verify_chain(&chain);
        for _ in 0..16 {
            assert_eq!(verify_chain(&chain), first);
        }
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&verify_chain(&chain)).expect("serialises")
        );
    }
}
