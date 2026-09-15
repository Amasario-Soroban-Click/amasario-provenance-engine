//! Typed dependency and provenance graphs: integrity, traversal, cycles and paths.
//!
//! # What this crate is for
//!
//! A resolved dependency set answers "what does this contract relate to". A graph answers
//! the questions that need the relationships *together*: what an entity reaches, what
//! reaches it, which entities are mutually dependent, and what can be published as the
//! artifact a consumer reconstructs provenance from. Those are different questions, and
//! this crate is where they are answered without re-deriving the relationships or
//! re-deciding what any of them mean.
//!
//! It sits above [`amasario_dependency`] and above [`amasario_core`], and it decides
//! nothing about what a dependency *is*. Every edge carries the dependency record it came
//! from, so a graph cannot hold a second opinion about a basis, a class or a verification
//! status.
//!
//! # The distinctions this crate exists to preserve
//!
//! **A dangling endpoint is a failure, not a warning.** `graph.schema.json` requires an
//! edge's endpoints to resolve to nodes in the same graph. A graph that looks complete
//! while containing an unresolvable relationship is worse than no graph, because a consumer
//! cannot tell.
//!
//! **An address is not an identity.** A node is identified by its entity kind and its
//! identifier together, because the artifact a contract was deployed from and the deployed
//! module are content-identical and are nonetheless two entities with different
//! relationships.
//!
//! # What this crate does not claim
//!
//! Amasario is provenance, dependency and impact infrastructure. It is not a security
//! scanner and no term here means "secure", "safe", "malicious" or "vulnerable". A
//! verified edge says the evidence supporting it was complete, not that either endpoint is
//! trustworthy.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod errors;
pub mod nodes;

// Re-exported at the crate root because they are used together and a caller should not have
// to know which module a concept lives in to name it.
pub use errors::{GraphFailure, describe as describe_failure, first_failure};
pub use nodes::{Node, NodeAttributes};

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{EntityKind, EntityRef};

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A re-export that went missing would otherwise be a breaking change discovered by a
        // downstream crate rather than here.
        let node =
            Node::for_entity(&EntityRef::new(EntityKind::Contract, "C-a").expect("a reference"));
        assert_eq!(node.id, "CONTRACT:C-a");
        assert!(NodeAttributes::default().is_empty());
        assert!(
            !GraphFailure::MissingGraphId
                .clone()
                .into_error()
                .to_string()
                .is_empty()
        );
        first_failure(Vec::new()).expect("nothing to report");
    }
}
