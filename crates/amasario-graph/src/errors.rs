//! The ways a graph can fail to be a graph, and how each is classified.
//!
//! # Why reference integrity is a failure and not a warning
//!
//! The specification's `graph.schema.json` states that an edge's `source` and
//! `target` "must resolve to a node in this graph". That is not a stylistic
//! preference: the graph is the artifact a consumer reconstructs provenance from, so
//! an edge whose endpoint has no node is a relationship the consumer cannot follow,
//! cannot attribute evidence to and cannot report. A partially resolvable graph is
//! worse than no graph, because it looks complete. Every variant here therefore turns
//! a silent dangling reference into a named failure.
//!
//! # Why the categories are not all `Graph`
//!
//! Most of these failures are genuinely about graph structure and are classified
//! [`ErrorCategory::Graph`]. Two are not, and pretending otherwise would hide the
//! distinction that the error model exists to preserve:
//!
//! * [`GraphFailure::UnknownEntityKind`] and [`GraphFailure::MalformedDocument`] are
//!   *validation* failures. They are detected before any graph exists, and a caller
//!   can act on them without knowing anything about graph theory.
//!
//! Specification compatibility is deliberately **not** part of this enum. A graph
//! document declares an `apiVersion` and a `specVersion`, and
//! [`amasario_core::engine::SpecificationStamp::check_compatible`] already decides what
//! this engine may interpret - one tested implementation of one rule. A second, local
//! version rule here would be a second place for the same policy to be subtly different,
//! and a document accepted by one and refused by the other.

use amasario_core::{EngineError, ErrorCategory, Result};

/// A way a graph can fail to be constructed, traversed or interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphFailure {
    /// An edge names an endpoint that is not a node in the graph.
    ///
    /// The one failure the specification names explicitly. `role` says which end of
    /// the edge dangled, because "source is missing" and "target is missing" call for
    /// different fixes: an unresolved source means the subject was never added, while
    /// an unresolved target usually means a dependency was recorded without the entity
    /// it points at.
    DanglingEdge {
        /// The edge that dangles, by identifier.
        edge: String,
        /// The identifier that did not resolve.
        endpoint: String,
        /// Which end of the edge it was: `source` or `target`.
        role: &'static str,
    },
    /// An edge's source and target are the same entity.
    ///
    /// `dependency/direct-dependency` rejects a self-dependency, and the dependency
    /// layer refuses one before it can reach a graph. A self-edge that arrives here
    /// therefore did not come from an observation, and accepting it would make the
    /// cycle detector report a one-node cycle as though it were real topology.
    SelfEdge {
        /// The entity that was both endpoints.
        entity: String,
    },
    /// Two nodes share an identifier.
    ///
    /// Node identifiers are what edges resolve against, so a duplicate makes every
    /// edge to that identifier ambiguous: a reader cannot tell which node's evidence
    /// applies.
    DuplicateNode {
        /// The identifier that appeared twice.
        id: String,
    },
    /// Two edges share an identifier.
    ///
    /// The specification requires an edge identifier to be "stable across runs for the
    /// same edge, so that a snapshot diff can tell an unchanged edge from a replaced
    /// one". If two edges share one, a diff cannot, and the stable identifier has
    /// stopped doing its job.
    DuplicateEdge {
        /// The identifier that appeared twice.
        id: String,
    },
    /// A relationship connects entity kinds it does not permit.
    ///
    /// `relationship-types` declares the subject and object kinds of every
    /// relationship, so this can only be reached by a hand-built edge. It is checked
    /// because a graph is a document that can be authored, not only produced.
    EndpointsNotPermitted {
        /// The relationship that was used.
        relationship: String,
        /// The kind of the subject.
        subject_kind: String,
        /// The kind of the object.
        object_kind: String,
    },
    /// An edge cites no evidence.
    ///
    /// `graph.schema.json` gives an edge's `evidence` array `minItems: 1`, so an
    /// unsupported edge is a schema violation rather than a weakly supported one.
    EdgeWithoutEvidence {
        /// The edge that cites nothing.
        edge: String,
    },
    /// A graph was built without an identifier.
    ///
    /// A graph identifier is what lets a snapshot, a diff and a report refer to the
    /// same artifact, so an empty one makes all three unable to name what they are
    /// about.
    MissingGraphId,
    /// A graph claims to be truncated without saying why.
    ///
    /// Ordered traversal must set its depth bound and say it was bounded, for the same
    /// reason the dependency layer requires it: a consumer cannot tell "no relationship
    /// exists" from "the search stopped" after the fact, so the analysis has to record
    /// which one happened.
    TruncationUnreported,
    /// A node names an entity kind that is not in the shared enumeration.
    UnknownEntityKind {
        /// The node the kind appeared on.
        node: String,
        /// The kind that was read.
        kind: String,
    },
    /// The document could not be read at all.
    MalformedDocument {
        /// What went wrong, in the parser's words.
        detail: String,
    },
}

impl GraphFailure {
    /// The category this failure belongs to.
    ///
    /// Deliberately not uniform. A dangling reference, a duplicate and a
    /// kind-incompatible edge are graph failures; unreadable input is a validation
    /// failure. Collapsing both into `Graph` would erase exactly the distinction
    /// [`ErrorCategory`] exists to carry.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::UnknownEntityKind { .. } | Self::MalformedDocument { .. } => {
                ErrorCategory::Validation
            },
            Self::DanglingEdge { .. }
            | Self::SelfEdge { .. }
            | Self::DuplicateNode { .. }
            | Self::DuplicateEdge { .. }
            | Self::EndpointsNotPermitted { .. }
            | Self::EdgeWithoutEvidence { .. }
            | Self::MissingGraphId
            | Self::TruncationUnreported => ErrorCategory::Graph,
        }
    }

    /// Whether the failure can be repaired by collecting more evidence.
    ///
    /// A dangling edge is the case this exists for: an unresolved target is often an
    /// entity the analysis has not inspected yet, so a later run with a wider boundary
    /// may resolve it. The remaining structural failures cannot be repaired by
    /// observation at all - they are statements about the document, not about the
    /// network - and saying so is what stops a caller retrying a typo against RPC.
    #[must_use]
    pub const fn is_repairable_by_observation(&self) -> bool {
        matches!(self, Self::DanglingEdge { .. })
    }

    /// Whether the failure means the input is unreadable rather than wrong.
    ///
    /// Distinct from the graph-structural failures on purpose: a caller receiving
    /// malformed JSON should report a bad document, not a broken graph.
    #[must_use]
    pub const fn is_malformed_input(&self) -> bool {
        matches!(
            self,
            Self::MalformedDocument { .. } | Self::UnknownEntityKind { .. }
        )
    }

    /// Converts the failure into the engine's structured error.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        match self {
            Self::DanglingEdge {
                edge,
                endpoint,
                role,
            } => EngineError::Graph(format!(
                "edge {edge} names a {role} node {endpoint:?} that is not in the graph; an edge \
                 whose endpoint has no node cannot be followed, and a graph that looks complete \
                 while containing one is worse than no graph"
            )),
            Self::SelfEdge { entity } => EngineError::Graph(format!(
                "the edge from {entity} to itself is not representable: a contract cannot depend \
                 on itself, and a one-node cycle would be reported as real topology"
            )),
            Self::DuplicateNode { id } => EngineError::Graph(format!(
                "node {id:?} appears more than once; edges resolve by identifier, so a duplicate \
                 makes every edge to it ambiguous"
            )),
            Self::DuplicateEdge { id } => EngineError::Graph(format!(
                "edge {id:?} appears more than once; a stable edge identifier is what lets a \
                 snapshot diff tell an unchanged edge from a replaced one"
            )),
            Self::EndpointsNotPermitted {
                relationship,
                subject_kind,
                object_kind,
            } => EngineError::Graph(format!(
                "{relationship} does not connect a {subject_kind} to a {object_kind}; \
                 relationship-types declares the endpoint kinds each relationship permits"
            )),
            Self::EdgeWithoutEvidence { edge } => EngineError::Graph(format!(
                "edge {edge} cites no evidence; the specification requires at least one record, so \
                 an unsupported edge is a schema violation rather than a weak claim"
            )),
            Self::MissingGraphId => EngineError::Graph(
                "a graph needs an identifier, because a snapshot, a diff and a report all refer to \
                 a graph by it"
                    .to_owned(),
            ),
            Self::TruncationUnreported => EngineError::Graph(
                "the graph says traversal was truncated but not why; a bounded search that does not \
                 say it was bounded invites the reader to conclude the relationship is absent"
                    .to_owned(),
            ),
            Self::UnknownEntityKind { node, kind } => EngineError::Validation {
                path: format!("/graph/nodes/{node}/kind"),
                detail: format!(
                    "{kind:?} is not an entity kind; the set is shared with \
                     graph.schema.json and node kinds and entity references cannot disagree \
                     about the vocabulary"
                ),
            },
            Self::MalformedDocument { detail } => EngineError::Validation {
                path: "/graph".to_owned(),
                detail: format!("the graph document could not be read: {detail}"),
            },
        }
    }
}

impl std::fmt::Display for GraphFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.clone().into_error().to_string())
    }
}

/// A one-line description of a failure, for a report or a summary.
#[must_use]
pub fn describe(failure: &GraphFailure) -> String {
    match failure {
        GraphFailure::DanglingEdge { endpoint, role, .. } => {
            format!("an edge names an unresolved {role} node {endpoint}")
        },
        GraphFailure::SelfEdge { entity } => format!("{entity} is related to itself"),
        GraphFailure::DuplicateNode { id } => format!("node {id} appears twice"),
        GraphFailure::DuplicateEdge { id } => format!("edge {id} appears twice"),
        GraphFailure::EndpointsNotPermitted {
            relationship,
            subject_kind,
            object_kind,
        } => format!("{relationship} cannot connect a {subject_kind} to a {object_kind}"),
        GraphFailure::EdgeWithoutEvidence { edge } => format!("edge {edge} cites no evidence"),
        GraphFailure::MissingGraphId => "the graph has no identifier".to_owned(),
        GraphFailure::TruncationUnreported => "the graph is truncated without a reason".to_owned(),
        GraphFailure::UnknownEntityKind { node, kind } => {
            format!("node {node} has unrecognised kind {kind}")
        },
        GraphFailure::MalformedDocument { detail } => format!("unreadable document: {detail}"),
    }
}

/// Reports the first failure in a list, or succeeds when there are none.
///
/// The same shape the dependency layer uses, so that an analysis which collects
/// problems per edge has one way to turn them into a result rather than each caller
/// inventing its own reduction.
///
/// # Errors
///
/// Returns the first failure as the engine's structured error.
pub fn first_failure(failures: Vec<GraphFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, so that the classification tests cannot silently stop covering
    /// one when a variant is added.
    fn every_failure() -> Vec<GraphFailure> {
        vec![
            GraphFailure::DanglingEdge {
                edge: "e1".to_owned(),
                endpoint: "CONTRACT:C-missing".to_owned(),
                role: "target",
            },
            GraphFailure::SelfEdge {
                entity: "CONTRACT:C-a".to_owned(),
            },
            GraphFailure::DuplicateNode {
                id: "CONTRACT:C-a".to_owned(),
            },
            GraphFailure::DuplicateEdge {
                id: "e1".to_owned(),
            },
            GraphFailure::EndpointsNotPermitted {
                relationship: "DEPLOYED_AS".to_owned(),
                subject_kind: "DEPLOYMENT".to_owned(),
                object_kind: "CONTRACT".to_owned(),
            },
            GraphFailure::EdgeWithoutEvidence {
                edge: "e1".to_owned(),
            },
            GraphFailure::MissingGraphId,
            GraphFailure::TruncationUnreported,
            GraphFailure::UnknownEntityKind {
                node: "n1".to_owned(),
                kind: "ORACLE".to_owned(),
            },
            GraphFailure::MalformedDocument {
                detail: "expected value".to_owned(),
            },
        ]
    }

    #[test]
    fn every_failure_has_a_category_a_description_and_a_code() {
        let failures = every_failure();
        for failure in &failures {
            assert_eq!(
                failure.category(),
                failure.clone().into_error().category(),
                "{failure:?} disagrees with its own error"
            );
            assert!(!describe(failure).is_empty());
            assert!(!failure.clone().into_error().code().is_empty());
        }
        assert_eq!(failures.len(), 10, "every variant is covered by this test");
    }

    #[test]
    fn the_categories_separate_structure_from_unreadable_input() {
        // The distinction the whole error model exists to carry, asserted rather than
        // assumed: a caller receiving malformed input should report a bad document, not a
        // broken graph. Compatibility is not here at all, because one tested
        // implementation of that rule already exists in `amasario-core`.
        for failure in every_failure() {
            let expected = match failure {
                GraphFailure::UnknownEntityKind { .. } | GraphFailure::MalformedDocument { .. } => {
                    ErrorCategory::Validation
                },
                _ => ErrorCategory::Graph,
            };
            assert_eq!(failure.category(), expected, "{failure:?}");
        }
        assert!(
            !every_failure()
                .iter()
                .any(|failure| failure.category() == ErrorCategory::SpecificationCompatibility),
            "version policy belongs to the engine's specification stamp, not to a graph failure"
        );
    }

    #[test]
    fn only_a_dangling_edge_can_be_repaired_by_observing_more() {
        for failure in every_failure() {
            let repairable = matches!(failure, GraphFailure::DanglingEdge { .. });
            assert_eq!(
                failure.is_repairable_by_observation(),
                repairable,
                "{failure:?} misclassified as repairable"
            );
        }
        // A duplicate node is a typo in a document; retrying it against RPC cannot
        // help, which is exactly what the predicate is for.
        assert!(
            !GraphFailure::DuplicateNode {
                id: "n1".to_owned()
            }
            .is_repairable_by_observation()
        );
    }

    #[test]
    fn malformed_input_is_distinguished_from_a_broken_graph() {
        assert!(
            GraphFailure::MalformedDocument {
                detail: "expected value".to_owned(),
            }
            .is_malformed_input()
        );
        assert!(
            !GraphFailure::DanglingEdge {
                edge: "e1".to_owned(),
                endpoint: "n1".to_owned(),
                role: "source",
            }
            .is_malformed_input()
        );
    }

    #[test]
    fn a_dangling_edge_says_which_end_dangled() {
        let source = GraphFailure::DanglingEdge {
            edge: "e1".to_owned(),
            endpoint: "CONTRACT:C-a".to_owned(),
            role: "source",
        }
        .into_error();
        assert!(source.to_string().contains("source"), "got: {source}");

        let target = GraphFailure::DanglingEdge {
            edge: "e1".to_owned(),
            endpoint: "CONTRACT:C-b".to_owned(),
            role: "target",
        }
        .into_error();
        assert!(target.to_string().contains("target"), "got: {target}");
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![GraphFailure::SelfEdge {
            entity: "CONTRACT:C-a".to_owned(),
        }])
        .expect_err("a failure must be reported");
        assert!(error.to_string().contains("cannot depend on itself"));
    }
}
