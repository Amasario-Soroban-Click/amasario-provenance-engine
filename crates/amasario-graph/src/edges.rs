//! Graph edges: one established relationship, carrying the dependency record it came
//! from.
//!
//! # Why an edge holds the whole dependency rather than a summary of it
//!
//! `graph.schema.json` opens its edge definition with a promise: the edge carries "the
//! same fields as a dependency edge so that graph and dependency views cannot diverge".
//! Two views of one fact drift the moment either of them restates it, so an [`Edge`]
//! does not restate anything. It owns the [`Dependency`] it came from, and every
//! accessor reads through to it. There is no place in this type for a second opinion
//! about the basis, the classes or the verification status, because a second opinion is
//! a second thing to keep in step.
//!
//! That is also why the schema *projection* lives in the serialization module rather
//! than here. The graph document the specification defines publishes a subset of the
//! dependency record - identifier, endpoints, relationship, evidence citations,
//! confidence, boundary and whether the edge was observed - and it deliberately does
//! not publish the basis, the classes or the verification status, because those belong
//! to the dependency document. Publishing a subset is a choice about a document, not a
//! property of an edge.
//!
//! # Why the identifier is derived rather than assigned
//!
//! The specification requires an edge identifier to be "stable across runs for the same
//! edge, so that a snapshot diff can tell an unchanged edge from a replaced one". An
//! assigned counter would satisfy neither half: it would change between runs, and it
//! would change when an unrelated edge was inserted earlier in the ordering.
//! [`Edge::stable_id`] derives it from the three things that *are* the edge - its
//! source, its relationship and its target - so re-running the analysis yields the same
//! identifier, and two edges differing in any of the three differ in theirs.
//!
//! A digest is used rather than the concatenated form because the schema bounds an edge
//! identifier at 256 characters, while a source identifier may be up to 512: a
//! repository URL plus a revision is legitimately long, and truncating it would
//! reintroduce the collision the identifier exists to prevent.

use amasario_core::{
    Basis, Confidence, DependencyClass, Digest, EntityRef, EvidenceType, Relationship,
    VerificationStatus,
};
use amasario_dependency::{Dependency, EvidenceRef};

/// One established relationship in a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    id: String,
    dependency: Dependency,
}

impl Edge {
    /// Builds the edge for a dependency, deriving its stable identifier.
    #[must_use]
    pub fn from_dependency(dependency: Dependency) -> Self {
        let id = Self::stable_id(
            &dependency.subject,
            dependency.relationship,
            &dependency.object,
        );
        Self { id, dependency }
    }

    /// The identifier this engine gives the edge between two entities.
    ///
    /// Deterministic, and dependent on exactly the three properties that constitute the
    /// edge, so that the identifier survives a re-run and changes whenever the edge
    /// does.
    #[must_use]
    pub fn stable_id(source: &EntityRef, relationship: Relationship, target: &EntityRef) -> String {
        // `\u{1f}` separates the parts. It is a unit separator that cannot appear in a
        // contract address, a digest, a revision or a relationship name, so no two
        // different triples can produce the same pre-image - which concatenation with a
        // printable delimiter could not guarantee.
        let pre_image = format!("{source}\u{1f}{}\u{1f}{target}", relationship.as_str());
        Digest::sha256_of(pre_image.as_bytes()).value().to_owned()
    }

    /// The edge's stable identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The dependency record the edge carries.
    #[must_use]
    pub const fn dependency(&self) -> &Dependency {
        &self.dependency
    }

    /// The entity the relationship starts at.
    #[must_use]
    pub const fn source(&self) -> &EntityRef {
        &self.dependency.subject
    }

    /// The entity the relationship points at.
    #[must_use]
    pub const fn target(&self) -> &EntityRef {
        &self.dependency.object
    }

    /// The relationship between the two entities.
    #[must_use]
    pub const fn relationship(&self) -> Relationship {
        self.dependency.relationship
    }

    /// How the dependency was established.
    #[must_use]
    pub const fn basis(&self) -> Basis {
        self.dependency.basis
    }

    /// The classes the evidence supports.
    #[must_use]
    pub fn classes(&self) -> &[DependencyClass] {
        &self.dependency.classes
    }

    /// How strongly the evidence supports the dependency, with its citations.
    #[must_use]
    pub const fn confidence(&self) -> &Confidence {
        &self.dependency.confidence
    }

    /// The evidence the dependency cites.
    #[must_use]
    pub fn evidence(&self) -> &[EvidenceRef] {
        &self.dependency.evidence
    }

    /// What the evidence says about the dependency.
    #[must_use]
    pub const fn verification(&self) -> VerificationStatus {
        self.dependency.verification
    }

    /// How many edges separate the subject from the target.
    #[must_use]
    pub const fn depth(&self) -> usize {
        self.dependency.depth
    }

    /// Whether the edge was directly observed rather than inferred.
    ///
    /// `graph.schema.json` gives its `observed` field this meaning, and the honest
    /// reading of it is the basis: `OBSERVED_INVOCATION` and `OBSERVED_EVENT` are
    /// observations, and `INFERRED_INTERFACE`, `DECLARED_MANIFEST`,
    /// `RESOLVED_LOCKFILE`, `EMBEDDED_DIGEST`, `CONFIGURED_ENDPOINT` and `ATTESTED`
    /// are not.
    ///
    /// Deliberately *not* [`Dependency::is_observed`], which answers a different
    /// question - whether a dependency may be reported among the observed facts, which
    /// also requires that the evidence was complete enough to verify it. A directly
    /// observed call whose transaction outcome was not recorded is still directly
    /// observed, and reporting it as inferred would understate what was seen.
    #[must_use]
    pub const fn is_directly_observed(&self) -> bool {
        self.dependency.basis.is_observed()
    }

    /// The evidence citations of one kind.
    ///
    /// Used by the report and export layers, which group citations by kind rather than
    /// listing them undifferentiated.
    #[must_use]
    pub fn evidence_of(&self, kind: EvidenceType) -> Vec<&str> {
        self.dependency
            .evidence
            .iter()
            .filter(|evidence| evidence.kind == kind)
            .map(|evidence| evidence.id.as_str())
            .collect()
    }

    /// The network the edge was established on, where one was recorded.
    #[must_use]
    pub fn network(&self) -> Option<&amasario_core::Network> {
        self.dependency.network()
    }

    /// Whether the edge lies in the direct partition of its set.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        self.dependency.depth == 0
    }

    /// The canonical order of two edges.
    ///
    /// By source identifier, then relationship in the taxonomy's own order, then target
    /// identifier. Ordering by the relationship's *rank* rather than its spelling keeps
    /// the order aligned with `relationship-types`, so two implementations that follow
    /// the taxonomy serialise the same bytes; ordering by the source first keeps every
    /// edge leaving one node adjacent, which is what makes a serialised adjacency list
    /// readable.
    #[must_use]
    pub fn canonical_order(left: &Self, right: &Self) -> std::cmp::Ordering {
        let source = left.source().to_string().cmp(&right.source().to_string());
        source
            .then_with(|| {
                relationship_rank(left.relationship()).cmp(&relationship_rank(right.relationship()))
            })
            .then_with(|| left.target().to_string().cmp(&right.target().to_string()))
    }
}

/// Renders an edge the way every layer renders one.
///
/// `"{source} -{RELATIONSHIP}-> {target}"`, which is the form the dependency layer's
/// closure already uses for the edges of a cycle. Kept in one place so that a cycle
/// reported by [`crate::cycles`], an edge listed by a traversal and an edge exported by
/// the report layer cannot be three spellings of one fact. `tests/cycle_conformance.rs`
/// compares this rendering against the dependency layer's own, so the duplication of the
/// format string between the two crates is checked rather than assumed.
#[must_use]
pub fn render(dependency: &Dependency) -> String {
    format!(
        "{} -{}-> {}",
        dependency.subject,
        dependency.relationship.as_str(),
        dependency.object
    )
}

/// A relationship's position in the shared enumeration.
///
/// Public because the published document orders its edges by the same rule, and two
/// orderings for one set of edges would make a serialised graph depend on which layer
/// wrote it.
#[must_use]
pub fn relationship_rank(relationship: Relationship) -> usize {
    Relationship::all()
        .iter()
        .position(|candidate| *candidate == relationship)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        EntityKind, EvidenceType, LedgerSequence, Network, NetworkType, ObservationBoundary,
    };
    use amasario_dependency::{Candidate, EvidenceRef, classify, resolve};

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a reference")
    }

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(4_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn dependency(from: &str, to: &str, relationship: Relationship) -> Dependency {
        let subject = entity(EntityKind::Contract, from);
        let candidate = Candidate::new(
            subject.clone(),
            entity(EntityKind::Contract, to),
            relationship,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true));
        let set = resolve(subject, Some(boundary()), &[candidate], 5).expect("resolves");
        set.direct.into_iter().next().expect("one dependency")
    }

    #[test]
    fn an_edge_reads_through_to_the_dependency_it_carries() {
        let dependency = dependency("C-a", "C-b", Relationship::Invocates);
        let edge = Edge::from_dependency(dependency.clone());
        assert_eq!(edge.source(), &dependency.subject);
        assert_eq!(edge.target(), &dependency.object);
        assert_eq!(edge.relationship(), Relationship::Invocates);
        assert_eq!(edge.basis(), dependency.basis);
        assert_eq!(edge.classes(), dependency.classes.as_slice());
        assert_eq!(edge.confidence(), &dependency.confidence);
        assert_eq!(edge.evidence(), dependency.evidence.as_slice());
        assert_eq!(edge.verification(), dependency.verification);
        assert_eq!(edge.depth(), 0);
        assert!(edge.is_direct());
    }

    #[test]
    fn an_observed_edge_reports_its_basis_not_its_verification() {
        // `observed` in graph.schema.json means "directly observed rather than
        // inferred", which is the basis. A directly observed call whose transaction
        // outcome was not recorded is still directly observed, and calling it inferred
        // would understate what was seen.
        let subject = entity(EntityKind::Contract, "C-a");
        let unknown_outcome = Candidate::new(
            subject.clone(),
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());
        assert!(unknown_outcome.successful.is_none());
        let set = resolve(subject, Some(boundary()), &[unknown_outcome], 5).expect("resolves");
        // The dependency layer refuses it, because an unknown outcome is not a
        // successful one - so the observation is reported, not the dependency.
        assert!(set.is_empty());
        assert_eq!(set.unestablished.len(), 1);

        let edge = Edge::from_dependency(dependency("C-a", "C-b", Relationship::Invocates));
        assert!(edge.is_directly_observed());

        // The predicate delegates to the basis's own classification rather than to a
        // second opinion, so it cannot drift from the vocabulary it reads.
        assert!(Basis::ObservedInvocation.is_observed());
        assert!(Basis::ObservedEvent.is_observed());
        assert!(!Basis::InferredInterface.is_observed());
        assert!(!Basis::DeclaredManifest.is_observed());
    }

    #[test]
    fn the_stable_identifier_depends_on_all_three_parts_of_the_edge() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let base = Edge::stable_id(&a, Relationship::Invocates, &b);

        assert_eq!(
            base,
            Edge::stable_id(&a, Relationship::Invocates, &b),
            "a re-run must produce the same identifier"
        );
        assert_ne!(
            base,
            Edge::stable_id(&a, Relationship::DependsOn, &b),
            "the relationship is part of the edge"
        );
        assert_ne!(
            base,
            Edge::stable_id(&b, Relationship::Invocates, &a),
            "the edge is directed"
        );
        assert_ne!(
            base,
            Edge::stable_id(
                &a,
                Relationship::Invocates,
                &entity(EntityKind::Contract, "C-c")
            ),
            "the target is part of the edge"
        );
        assert_ne!(
            base,
            Edge::stable_id(
                &a,
                Relationship::Invocates,
                &entity(EntityKind::Artifact, "C-b")
            ),
            "the same identifier under another kind is another entity"
        );
    }

    #[test]
    fn the_stable_identifier_fits_the_schemas_bound() {
        // The schema bounds an edge identifier at 256 characters while a source
        // identifier may be 512, so a concatenated form would not fit and a truncated
        // one would collide. A digest fits and does not.
        let long = entity(
            EntityKind::Source,
            &format!(
                "https://example.invalid/{}@{}",
                "r".repeat(200),
                "f".repeat(200)
            ),
        );
        let id = Edge::stable_id(
            &long,
            Relationship::BuiltFrom,
            &entity(EntityKind::Build, "b"),
        );
        assert_eq!(id.len(), 64);
        assert!(id.len() <= 256);
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        );
    }

    #[test]
    fn the_identifier_is_the_same_whether_it_is_derived_from_the_parts_or_the_edge() {
        let dependency = dependency("C-a", "C-b", Relationship::Invocates);
        let edge = Edge::from_dependency(dependency.clone());
        assert_eq!(
            edge.id(),
            Edge::stable_id(
                &dependency.subject,
                dependency.relationship,
                &dependency.object
            )
        );
    }

    #[test]
    fn edges_are_ordered_by_source_then_relationship_then_target() {
        let mut edges = [
            Edge::from_dependency(dependency("C-b", "C-a", Relationship::Invocates)),
            Edge::from_dependency(dependency("C-a", "C-c", Relationship::DependsOn)),
            Edge::from_dependency(dependency("C-a", "C-a2", Relationship::DependsOn)),
            Edge::from_dependency(dependency("C-a", "C-b", Relationship::Invocates)),
        ];
        edges.sort_by(Edge::canonical_order);
        let rendered: Vec<String> = edges
            .iter()
            .map(|edge| {
                format!(
                    "{} -{}-> {}",
                    edge.source(),
                    edge.relationship().as_str(),
                    edge.target()
                )
            })
            .collect();
        assert_eq!(
            rendered,
            vec![
                "CONTRACT:C-a -DEPENDS_ON-> CONTRACT:C-a2",
                "CONTRACT:C-a -DEPENDS_ON-> CONTRACT:C-c",
                "CONTRACT:C-a -INVOCATES-> CONTRACT:C-b",
                "CONTRACT:C-b -INVOCATES-> CONTRACT:C-a",
            ]
        );
    }

    #[test]
    fn citations_are_grouped_by_kind_rather_than_listed_undifferentiated() {
        let subject = entity(EntityKind::Contract, "C-a");
        let candidate = Candidate::new(
            subject.clone(),
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![
                EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation"),
                EvidenceRef::new(EvidenceType::Event, "e1").expect("a citation"),
                EvidenceRef::new(EvidenceType::Event, "e2").expect("a citation"),
            ],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true));
        let set = resolve(subject, Some(boundary()), &[candidate], 5).expect("resolves");
        let edge = Edge::from_dependency(set.direct.into_iter().next().expect("one dependency"));
        assert_eq!(edge.evidence_of(EvidenceType::Event).len(), 2);
        assert_eq!(edge.evidence_of(EvidenceType::Transaction).len(), 1);
        assert!(edge.evidence_of(EvidenceType::Attestation).is_empty());
        assert_eq!(
            edge.network().map(|network| network.id.as_str()),
            Some("testnet")
        );
    }

    #[test]
    fn classification_of_an_edge_that_the_layer_refuses_never_reaches_a_graph() {
        // classify is the gate; this test records that the gate is upstream of Edge, so
        // an Edge cannot exist for a relationship the classifier refused.
        let subject = entity(EntityKind::Contract, "C-a");
        let refused = Candidate::new(
            subject,
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::DeclaredManifest,
            vec![EvidenceRef::new(EvidenceType::Source, "s1").expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());
        assert!(classify(&refused).is_err());
    }
}
