//! Impact analysis: what a change reaches, and on what evidence.
//!
//! # What this crate answers
//!
//! Given a typed provenance graph and a change to one entity, which other entities could
//! be affected, how far away are they, along which route, and what supports each step of
//! that route. It is the third question Amasario formalizes - after "what does this
//! contract depend on" and "where did its artifact come from" - and it is the one whose
//! answer is most easily overstated, which is why the whole crate is built around
//! refusing to overstate it.
//!
//! # The four things a finding is not allowed to do
//!
//! Each is a rule from `rules/impact/`, and each is enforced in code rather than
//! documented and hoped for:
//!
//! 1. **It cannot invent a direction.** `taxonomies/relationship-types.yaml` declares a
//!    `changePropagation` for every relationship and states that impact "is derived from
//!    `changePropagation`; it is never inferred from the name of the relationship."
//!    [`propagation`] is that derivation. It matters because `DEPENDS_ON` reads
//!    subject-first while a change to its *object* is what reaches its subject: a
//!    name-based implementation propagates the wrong way, and would report that changing
//!    a dependent breaks its dependency.
//! 2. **It cannot traverse a non-propagating relationship.** `OBSERVED_IN` and
//!    `VERIFIED_BY` have `changePropagation: none`, and
//!    [`ImpactFailure::NonPropagatingStep`] makes traversing one a named refusal rather
//!    than a silent skip, so an empty result cannot hide a declined question.
//! 3. **It cannot manufacture confidence.** A path's aggregate is the minimum ordinal
//!    across its steps, and [`ImpactFailure::ConfidenceNotWeakestLink`] refuses a finding
//!    whose aggregate is stronger than its weakest step.
//! 4. **It cannot hide its path.** A finding at any non-zero depth carries the route it
//!    took, with per-step evidence, so a reader who disagrees can name the hop they
//!    dispute instead of rejecting a number.
//!
//! # What a finding is
//!
//! [`ImpactFinding`] is the specification's record: the changed entity, the affected
//! entity, the path, the hop count, the relationship types in path order, the evidence,
//! the confidence, the reason, the change type and the verification state. Its
//! classifications are *derived* - distance terms from the hop depth, an entity term from
//! the affected entity's kind, `CHANGE` from whether a change type was supplied - so a
//! producer cannot attach a term the depth does not support.
//!
//! # The sentence that must not be forgotten
//!
//! `rules/impact/direct-impact.yaml` states the non-goal plainly: "This rule does not
//! claim the affected entity is broken, only that a change may reach it." An affected
//! entity is a claim about correspondence, never about correctness, safety or
//! vulnerability. Nothing in this crate computes or implies the second kind of claim, and
//! `SECURITY.md` says why the engine refuses to.

pub mod affected;
pub mod errors;
pub mod propagation;

// Re-exported together because they are used together: a caller reasoning about an
// impact finding needs the model, the failures it can have, and the traversal vocabulary
// it is expressed in.
pub use affected::{
    ChangeType, DeploymentRecord, ImpactFinding, ImpactPath, ImpactType, ImpactTypeFamily,
    PathTermination, aggregate_confidence, path_evidence, required_distance_terms,
    required_entity_term,
};
pub use errors::{ImpactFailure, describe as describe_failure, first_failure};
pub use propagation::{
    ImpactContext, ImpactDirection, Step, StepDirection, combine_directions, describe_route,
    non_propagation_reason, propagates, propagates_backward, propagates_forward, step_direction,
};

// Re-exported so that a consumer does not have to depend on `amasario-provenance` merely
// to ask whether a deployment can be affected, which is an impact question.
pub use amasario_provenance::DeploymentStatus;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name it.
        // A re-export that went missing would otherwise be a breaking change discovered
        // downstream rather than here.
        assert_eq!(ImpactType::Direct.as_str(), "DIRECT");
        assert_eq!(ImpactType::Transitive.family(), ImpactTypeFamily::Distance);
        assert_eq!(StepDirection::Inverted.as_str(), "inverted");
        assert_eq!(ImpactDirection::Dependents.as_str(), "DEPENDENTS");
        assert_eq!(
            PathTermination::NoPropagatingEdge.as_str(),
            "NO_PROPAGATING_EDGE"
        );
        assert_eq!(ChangeType::Replaced.as_str(), "REPLACED");
        assert_eq!(DeploymentStatus::Confirmed.as_str(), "CONFIRMED");
        assert!(
            describe_failure(&ImpactFailure::PathRepeatsEntity {
                entity: "C-a".to_owned(),
            })
            .contains("cycle")
        );
    }
}
