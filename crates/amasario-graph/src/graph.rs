//! The graph: typed nodes, typed edges, and the integrity rules that make it usable.
//!
//! # Why a transitive dependency is not an edge
//!
//! A [`amasario_dependency::DependencySet`] carries two kinds of statement. A direct
//! dependency is one edge; a transitive dependency is a claim that the subject
//! *reaches* the object through intermediates. Turning the second into an edge would
//! invent a relationship that was never observed - a shortcut from `A` to `C` - and
//! would misreport the topology: a consumer reading the edge would conclude `A`
//! directly invokes `C`, which is exactly the confusion
//! `dependency/transitive-dependency` exists to prevent. The reachability claim is
//! therefore kept as a **derived** record carrying its path, and the graph's edges
//! remain the relationships that were actually established.
//!
//! # Why refusals travel with the graph
//!
//! A dependency set records the candidates that could not become dependencies, with
//! the reason for each. Those are carried through rather than dropped, because a report
//! that receives only the graph would otherwise have to describe the analysis as
//! complete when something was seen and refused. The graph is the input to impact
//! analysis and reporting; the refusals have to reach both.
//!
//! # Why validation is not optional
//!
//! A `Graph` can be assembled by hand - that is the point of it being a public
//! structure rather than only a product of [`Graph::from_dependencies`] - and a
//! hand-assembled graph can contain an edge whose endpoint was never added,
//! a relationship connecting kinds it does not permit, or two nodes claiming one
//! identifier. `graph.schema.json` states the resolution requirement as a rule rather
//! than a convention, so [`Graph::validate`] is where it is enforced, and every
//! constructor calls it before returning.

use amasario_core::{
    EntityRef, Network, ObservationBoundary, Relationship, Result, TruncationReason,
};
use amasario_dependency::{Dependency, DependencySet, EdgeSource, Unestablished};

use crate::edges::Edge;
use crate::errors::GraphFailure;
use crate::nodes::{Node, NodeAttributes, canonical_order};

/// A typed, directed graph of Amasario entities and relationships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graph {
    /// The graph's identifier within the document that produced it.
    pub id: String,
    /// The observation boundary the graph was assembled at, where one was recorded.
    pub boundary: Option<ObservationBoundary>,
    /// The traversal depth the graph was bounded by, where it was bounded.
    pub max_depth: Option<usize>,
    /// Whether traversal stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why traversal stopped, when it did.
    ///
    /// Carried rather than recomputed, because the reason belongs to the traversal that
    /// produced the bound and is not recoverable from the topology: a graph truncated at
    /// its depth bound and one truncated by a rate limit have the same edges.
    pub truncation_reason: Option<TruncationReason>,
    /// The graph's nodes, in canonical order once [`Graph::canonicalise`] has run.
    pub nodes: Vec<Node>,
    /// The relationships that were established, in canonical order once canonicalised.
    pub edges: Vec<Edge>,
    /// Reachability claims: transitive dependencies, each carrying its path.
    ///
    /// Not edges. A derived record says the subject reaches the object; the edges say
    /// how a single relationship connects two entities, and merging the two would
    /// report a path as though it were one observation.
    pub derived: Vec<Dependency>,
    /// Candidates that could not become dependencies, with the reason for each.
    pub unestablished: Vec<Unestablished>,
}

impl Graph {
    /// Builds an empty graph.
    ///
    /// # Errors
    ///
    /// Returns a graph error when the identifier is empty: a snapshot, a diff and a
    /// report all refer to a graph by it.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(GraphFailure::MissingGraphId.into_error());
        }
        Ok(Self {
            id,
            boundary: None,
            max_depth: None,
            truncated: false,
            truncation_reason: None,
            nodes: Vec::new(),
            edges: Vec::new(),
            derived: Vec::new(),
            unestablished: Vec::new(),
        })
    }

    /// Builds a graph from a resolved dependency set.
    ///
    /// Direct dependencies become edges; transitive dependencies become derived
    /// reachability claims. Every entity named by either - including the intermediates
    /// on a path - becomes a node, so that no edge or claim can dangle.
    ///
    /// # Errors
    ///
    /// Returns a graph error when the assembled graph violates an integrity rule, or
    /// when the set itself does not validate.
    pub fn from_dependencies(id: impl Into<String>, set: &DependencySet) -> Result<Self> {
        set.validate()?;
        let mut graph = Self::new(id)?;

        for dependency in set.all() {
            graph.add_node(Node::for_entity(&dependency.subject))?;
            graph.add_node(Node::for_entity(&dependency.object))?;
            for intermediate in &dependency.path {
                graph.add_node(Node::for_entity(intermediate))?;
            }
        }

        for dependency in &set.direct {
            graph.add_edge(dependency.clone())?;
        }
        for dependency in &set.transitive {
            graph.derived.push(dependency.clone());
        }

        graph.boundary.clone_from(&set.boundary);
        graph.max_depth = Some(set.max_depth);
        graph.truncated = set.truncated;
        graph.truncation_reason = set.truncation_reason;
        graph.unestablished = set.unestablished.clone();
        graph.canonicalise();
        graph.validate()?;
        Ok(graph)
    }

    /// Builds a graph from a flat list of established dependencies.
    ///
    /// The convenience constructor for a caller that holds edges and not a set - the
    /// integration tests and the fixture loaders do. A dependency at depth zero becomes
    /// an edge; anything deeper becomes a derived claim, for the same reason as
    /// [`Graph::from_dependencies`].
    ///
    /// # Errors
    ///
    /// Returns a graph error when the assembled graph violates an integrity rule.
    pub fn from_dependencies_flat(
        id: impl Into<String>,
        dependencies: impl IntoIterator<Item = Dependency>,
    ) -> Result<Self> {
        let mut graph = Self::new(id)?;
        for dependency in dependencies {
            graph.add_node(Node::for_entity(&dependency.subject))?;
            graph.add_node(Node::for_entity(&dependency.object))?;
            for intermediate in &dependency.path {
                graph.add_node(Node::for_entity(intermediate))?;
            }
            if dependency.depth == 0 {
                graph.add_edge(dependency)?;
            } else {
                graph.derived.push(dependency);
            }
        }
        graph.canonicalise();
        graph.validate()?;
        Ok(graph)
    }

    /// Records the observation boundary the graph was assembled at.
    #[must_use]
    pub fn with_boundary(mut self, boundary: ObservationBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Records the traversal bound the graph was assembled under.
    #[must_use]
    pub const fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = Some(max_depth);
        self
    }

    /// Records that traversal stopped before exhausting what is reachable, and why.
    ///
    /// The reason and the flag are set together rather than separately, because a graph
    /// claiming truncation without a reason is refused by [`Graph::validate`] - so an API
    /// that allowed the two to be set apart would offer a way to build an invalid graph.
    #[must_use]
    pub const fn truncated_by(mut self, reason: TruncationReason) -> Self {
        self.truncated = true;
        self.truncation_reason = Some(reason);
        self
    }

    /// Adds a node, ignoring one that is already present.
    ///
    /// Adding is idempotent because two edges that share an endpoint both cause it to be
    /// added, and refusing the second would make assembly order matter. A node that
    /// already exists is *not* overwritten: silently replacing attributes would lose
    /// whichever facts arrived first, and merging two descriptions of one entity is a
    /// decision the caller has to make explicitly.
    ///
    /// # Errors
    ///
    /// Returns a graph error when a different node already claims the identifier.
    pub fn add_node(&mut self, node: Node) -> Result<()> {
        match self.nodes.iter().find(|existing| existing.id == node.id) {
            Some(existing) if *existing == node => Ok(()),
            Some(existing) => Err(GraphFailure::DuplicateNode {
                id: existing.id.clone(),
            }
            .into_error()),
            None => {
                self.nodes.push(node);
                Ok(())
            },
        }
    }

    /// Adds the node for an entity reference, ignoring one that is already present.
    ///
    /// # Errors
    ///
    /// Returns a graph error when a different node already claims the identifier.
    pub fn add_entity(&mut self, entity: &EntityRef) -> Result<()> {
        self.add_node(Node::for_entity(entity))
    }

    /// Records an edge for a dependency, deriving its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns a graph error when an edge with the same three parts already exists, when
    /// the dependency is not a direct one, or when the endpoints do not resolve to nodes
    /// in this graph. The last check is why the endpoints are added first in every
    /// constructor: an edge that dangles is refused rather than repaired, because
    /// repairing it by inventing a node would fabricate an entity.
    pub fn add_edge(&mut self, dependency: Dependency) -> Result<()> {
        if dependency.depth != 0 {
            return Err(GraphFailure::DuplicateEdge {
                id: format!(
                    "depth-{} dependency {} -> {}",
                    dependency.depth, dependency.subject, dependency.object
                ),
            }
            .into_error());
        }
        let edge = Edge::from_dependency(dependency);
        if self.edges.iter().any(|existing| existing.id() == edge.id()) {
            return Err(GraphFailure::DuplicateEdge {
                id: edge.id().to_owned(),
            }
            .into_error());
        }
        self.add_entity(edge.source())?;
        self.add_entity(edge.target())?;
        self.edges.push(edge);
        Ok(())
    }

    /// The graph's node count.
    #[must_use]
    pub const fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// The graph's edge count.
    #[must_use]
    pub const fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Whether the graph has no nodes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The network the graph was assembled on, where one was recorded.
    #[must_use]
    pub fn network(&self) -> Option<&Network> {
        self.boundary.as_ref().map(|boundary| &boundary.network)
    }

    /// The node with an identifier.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// The node for an entity reference.
    #[must_use]
    pub fn node_for(&self, entity: &EntityRef) -> Option<&Node> {
        self.node(&entity.to_string())
    }

    /// Whether the graph has a node for an entity.
    #[must_use]
    pub fn contains(&self, entity: &EntityRef) -> bool {
        self.node_for(entity).is_some()
    }

    /// The edges leaving an entity.
    #[must_use]
    pub fn edges_from(&self, entity: &EntityRef) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|edge| edge.source() == entity)
            .collect()
    }

    /// The edges arriving at an entity.
    #[must_use]
    pub fn edges_to(&self, entity: &EntityRef) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|edge| edge.target() == entity)
            .collect()
    }

    /// The edges connecting two entities in a stated direction.
    #[must_use]
    pub fn edges_between(&self, source: &EntityRef, target: &EntityRef) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|edge| edge.source() == source && edge.target() == target)
            .collect()
    }

    /// The entities an entity relates to directly.
    #[must_use]
    pub fn neighbours(&self, entity: &EntityRef) -> Vec<&EntityRef> {
        self.edges_from(entity)
            .into_iter()
            .map(Edge::target)
            .collect()
    }

    /// Nodes with no edges: discovered, but not related by anything.
    ///
    /// Reported explicitly rather than omitted, because `graph.schema.json` says why: a
    /// node "that was discovered but could not be related is a finding in itself, and
    /// dropping it would make the graph look tidier than the analysis was". A node that
    /// only ever appears as an endpoint of a derived claim has no *edge*, which is
    /// exactly the kind of node that must not disappear.
    #[must_use]
    pub fn disconnected_nodes(&self) -> Vec<&str> {
        self.nodes
            .iter()
            .filter(|node| {
                !self.edges.iter().any(|edge| {
                    edge.source().to_string() == node.id || edge.target().to_string() == node.id
                })
            })
            .map(|node| node.id.as_str())
            .collect()
    }

    /// Nodes no edge points at.
    #[must_use]
    pub fn roots(&self) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| {
                !self
                    .edges
                    .iter()
                    .any(|edge| edge.target().to_string() == node.id)
            })
            .collect()
    }

    /// Nodes no edge leaves.
    #[must_use]
    pub fn leaves(&self) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| {
                !self
                    .edges
                    .iter()
                    .any(|edge| edge.source().to_string() == node.id)
            })
            .collect()
    }

    /// Marks a node as lying outside the observable boundary.
    ///
    /// # Errors
    ///
    /// Returns a graph error when no node has the entity's identifier, because marking an
    /// entity that is not in the graph would silently do nothing.
    pub fn mark_external(&mut self, entity: &EntityRef, external: bool) -> Result<()> {
        let id = entity.to_string();
        let node = self
            .nodes
            .iter_mut()
            .find(|node| node.id == id)
            .ok_or_else(|| {
                GraphFailure::DanglingEdge {
                    edge: "(node attribute)".to_owned(),
                    endpoint: id.clone(),
                    role: "source",
                }
                .into_error()
            })?;
        let mut attributes = node.attributes.clone().unwrap_or_default();
        attributes.external = Some(external);
        node.attributes = Some(attributes);
        Ok(())
    }

    /// Records the network on every node, from the graph's boundary.
    ///
    /// Convenience for the common case where every entity in a graph was observed on
    /// one network. Deliberately not automatic: a graph assembled from two networks has
    /// no single answer, and applying one silently would be a false statement about
    /// half its nodes.
    pub fn annotate_network(&mut self) {
        let Some(network_id) = self.network().map(|network| network.id.clone()) else {
            return;
        };
        for node in &mut self.nodes {
            let mut attributes = node.attributes.clone().unwrap_or_default();
            attributes.network_id = Some(network_id.clone());
            node.attributes = Some(attributes);
        }
    }

    /// Sorts nodes and edges into their canonical order.
    ///
    /// Called by every constructor and available to a caller that assembled a graph by
    /// hand. Ordering is what makes serialisation deterministic, and the schema requires
    /// a graph to be reproducible: two runs over the same evidence must produce the same
    /// bytes, or a snapshot diff reports every edge as changed.
    pub fn canonicalise(&mut self) {
        self.nodes.sort_by(canonical_order);
        self.edges.sort_by(Edge::canonical_order);
        self.derived.sort_by(|left, right| {
            left.subject
                .to_string()
                .cmp(&right.subject.to_string())
                .then_with(|| left.depth.cmp(&right.depth))
                .then_with(|| left.object.to_string().cmp(&right.object.to_string()))
        });
        self.unestablished.sort_by(|left, right| {
            left.object
                .id
                .cmp(&right.object.id)
                .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
        });
    }

    /// Whether a relationship between two entity kinds is representable in this graph.
    ///
    /// Exposed because a caller assembling edges by hand asks this question before it
    /// builds one, and because the answer is the taxonomy's rather than a local rule.
    #[must_use]
    pub fn permits(relationship: Relationship, source: &EntityRef, target: &EntityRef) -> bool {
        relationship.permits(source.kind, target.kind)
    }

    /// Checks every integrity rule the specification states about a graph.
    ///
    /// Collects all failures rather than stopping at the first, so that a caller fixing
    /// a hand-assembled graph sees the whole list; [`crate::errors::first_failure`] turns
    /// the list into a result.
    ///
    /// # Errors
    ///
    /// Returns the first violated rule as the engine's structured error.
    pub fn validate(&self) -> Result<()> {
        crate::errors::first_failure(self.integrity_failures())
    }

    /// Every integrity rule the graph violates, in a deterministic order.
    ///
    /// Separate from [`Graph::validate`] so that a report can list the problems instead
    /// of reporting one, which is what makes a graph with several dangling references
    /// fixable in one pass.
    #[must_use]
    pub fn integrity_failures(&self) -> Vec<GraphFailure> {
        let mut failures = Vec::new();

        if self.id.is_empty() {
            failures.push(GraphFailure::MissingGraphId);
        }
        if self.truncated && self.truncation_reason.is_none() {
            failures.push(GraphFailure::TruncationUnreported);
        }
        if !self.truncated && self.truncation_reason.is_some() {
            failures.push(GraphFailure::TruncationUnreported);
        }

        for (index, node) in self.nodes.iter().enumerate() {
            if self.nodes[..index].iter().any(|seen| seen.id == node.id) {
                failures.push(GraphFailure::DuplicateNode {
                    id: node.id.clone(),
                });
            }
            // The identifier carries the kind, so a node whose first segment disagrees
            // with its kind field cannot be resolved consistently by an edge.
            let prefix = format!("{}:", node.kind.as_str());
            if !node.id.starts_with(&prefix) {
                failures.push(GraphFailure::UnknownEntityKind {
                    node: node.id.clone(),
                    kind: node.kind.as_str().to_owned(),
                });
            }
        }

        for (index, edge) in self.edges.iter().enumerate() {
            if self.edges[..index]
                .iter()
                .any(|seen| seen.id() == edge.id())
            {
                failures.push(GraphFailure::DuplicateEdge {
                    id: edge.id().to_owned(),
                });
            }
            if edge.source() == edge.target() {
                failures.push(GraphFailure::SelfEdge {
                    entity: edge.source().to_string(),
                });
            }
            if !edge
                .relationship()
                .permits(edge.source().kind, edge.target().kind)
            {
                failures.push(GraphFailure::EndpointsNotPermitted {
                    relationship: edge.relationship().as_str().to_owned(),
                    subject_kind: edge.source().kind.as_str().to_owned(),
                    object_kind: edge.target().kind.as_str().to_owned(),
                });
            }
            if edge.evidence().is_empty() {
                failures.push(GraphFailure::EdgeWithoutEvidence {
                    edge: edge.id().to_owned(),
                });
            }
            for (role, endpoint) in [("source", edge.source()), ("target", edge.target())] {
                match self.node_for(endpoint) {
                    Some(node) if node.kind == endpoint.kind => {},
                    _ => failures.push(GraphFailure::DanglingEdge {
                        edge: edge.id().to_owned(),
                        endpoint: endpoint.to_string(),
                        role,
                    }),
                }
            }
        }

        // A derived claim is not an edge, but it names entities, and an entity that
        // cannot be resolved is a reachability claim the consumer cannot follow.
        for claim in &self.derived {
            for (role, endpoint) in [("source", &claim.subject), ("target", &claim.object)] {
                if self.node_for(endpoint).is_none() {
                    failures.push(GraphFailure::DanglingEdge {
                        edge: format!("derived({})", claim.reason),
                        endpoint: endpoint.to_string(),
                        role,
                    });
                }
            }
            for intermediate in &claim.path {
                if self.node_for(intermediate).is_none() {
                    failures.push(GraphFailure::DanglingEdge {
                        edge: format!("derived({})", claim.reason),
                        endpoint: intermediate.to_string(),
                        role: "intermediate",
                    });
                }
            }
        }

        failures
    }

    /// Whether every node carries the network the graph was observed on.
    ///
    /// A structural question the export layer asks before publishing a graph whose
    /// boundary is recorded, so that a consumer does not have to infer the network from
    /// the boundary for every node in turn.
    #[must_use]
    pub fn nodes_are_annotated(&self) -> bool {
        self.nodes.iter().all(|node| {
            node.attributes
                .as_ref()
                .and_then(|attributes| attributes.network_id.as_ref())
                .is_some()
        })
    }

    /// The attributes recorded for a node, where any were.
    #[must_use]
    pub fn attributes(&self, id: &str) -> Option<&NodeAttributes> {
        self.node(id).and_then(|node| node.attributes.as_ref())
    }
}

impl EdgeSource for Graph {
    /// The edges leaving an entity, as direct dependencies.
    ///
    /// Delegates the filtering and the ordering to the dependency layer's own
    /// implementation, so that a closure over a graph visits entities in exactly the
    /// order it would visit them over the set the graph came from. Two orderings for one
    /// topology would make `close` and `close_set` disagree about which path is
    /// shortest, and the reported path is part of the result.
    fn edges_from(&self, entity: &EntityRef) -> Vec<Dependency> {
        let dependencies: Vec<Dependency> = self
            .edges
            .iter()
            .filter(|edge| edge.source() == entity)
            .map(|edge| edge.dependency().clone())
            .collect();
        dependencies.as_slice().edges_from(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EvidenceType, LedgerSequence, NetworkType, VerificationStatus,
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
            ledger: LedgerSequence::new(5_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn invocation(from: &str, to: &str, transaction: &str) -> Candidate {
        Candidate::new(
            entity(EntityKind::Contract, from),
            entity(EntityKind::Contract, to),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    fn set_of(candidates: &[Candidate]) -> DependencySet {
        resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            candidates,
            5,
        )
        .expect("resolves")
    }

    /// Recasts a resolved direct dependency as a reachability claim.
    ///
    /// The `DIRECT` class is removed rather than merely added to, because a dependency
    /// cannot be one edge from the subject and several hops away at once, and the set
    /// validator refuses a claim carrying both.
    fn promote_to_transitive(mut dependency: Dependency, path: Vec<EntityRef>) -> Dependency {
        dependency
            .classes
            .retain(|class| *class != amasario_core::DependencyClass::Direct);
        dependency.depth = path.len() + 1;
        dependency.path = path;
        dependency.with_class(amasario_core::DependencyClass::Transitive)
    }

    #[test]
    fn a_graph_built_from_a_set_carries_every_entity_either_partition_names() {
        let set = set_of(&[
            invocation("C-subject", "C-a", &"a".repeat(64)),
            invocation("C-subject", "C-b", &"b".repeat(64)),
        ]);
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        assert_eq!(graph.edge_count(), 2);
        assert_eq!(graph.node_count(), 3);
        assert!(graph.contains(&entity(EntityKind::Contract, "C-subject")));
        assert!(graph.contains(&entity(EntityKind::Contract, "C-a")));
        assert!(graph.validate().is_ok());
        assert!(graph.disconnected_nodes().is_empty());
        assert_eq!(graph.max_depth, Some(5));
        assert!(!graph.truncated);
        assert_eq!(
            graph.network().map(|network| network.id.as_str()),
            Some("testnet")
        );
    }

    #[test]
    fn a_transitive_dependency_becomes_a_reachability_claim_not_an_edge() {
        // The distinction the module exists to preserve: an edge from the subject to a
        // depth-two target would tell a consumer the subject directly relates to it,
        // which the analysis never established.
        let wide = DependencySet {
            subject: entity(EntityKind::Contract, "C-subject"),
            boundary: Some(boundary()),
            max_depth: 5,
            direct: set_of(&[
                invocation("C-subject", "C-a", &"a".repeat(64)),
                invocation("C-a", "C-deep", &"c".repeat(64)),
            ])
            .direct,
            transitive: vec![{
                promote_to_transitive(
                    set_of(&[invocation("C-subject", "C-far", &"c".repeat(64))])
                        .direct
                        .remove(0),
                    vec![entity(EntityKind::Contract, "C-a")],
                )
            }],
            unestablished: Vec::new(),
            cycles: Vec::new(),
            truncated: false,
            truncation_reason: None,
        };
        // The set is refused by its own validator only if it contradicts itself, so this
        // fixture is assembled from real edges plus one well-formed claim.
        wide.validate().expect("the fixture is a valid set");
        let graph = Graph::from_dependencies("g1", &wide).expect("a graph");
        assert_eq!(graph.edge_count(), 2, "one edge per direct dependency");
        assert_eq!(graph.derived.len(), 1, "one reachability claim");
        assert_eq!(graph.derived[0].depth, 2);
        assert_eq!(graph.derived[0].path.len(), 1);
        assert_eq!(graph.derived[0].object.id, "C-far");
        assert!(
            graph.contains(&entity(EntityKind::Contract, "C-a")),
            "the intermediate on the path must be a node, or the claim dangles"
        );
        assert!(
            !graph.edges.iter().any(|edge| edge.target().id == "C-far"),
            "the reachability claim must not become an edge"
        );
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn a_dangling_edge_is_refused_rather_than_repaired() {
        let mut graph = Graph::new("g1").expect("a graph");
        // Removing the node after the edge was added is how a hand-assembled graph ends
        // up with a dangling reference, and it is exactly what must not be serialised.
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        graph.edges = vec![Edge::from_dependency(set.direct[0].clone())];
        graph.nodes.clear();
        let failures = graph.integrity_failures();
        assert_eq!(failures.len(), 2, "both ends dangle: {failures:?}");
        assert!(
            failures
                .iter()
                .all(GraphFailure::is_repairable_by_observation)
        );
        let error = graph.validate().expect_err("a dangling edge is refused");
        assert!(
            error.to_string().contains("not in the graph"),
            "got: {error}"
        );
    }

    #[test]
    fn a_relationship_connecting_impermissible_kinds_is_refused() {
        // `DEPLOYED_AS` declares CONTRACT, WASM and ARTIFACT as its subjects, so a
        // DEPLOYMENT subject is not expressible - which is the check that caught a
        // mis-encoded hop in the provenance crate.
        assert!(!Relationship::DeployedAs.permits(EntityKind::Deployment, EntityKind::Contract));
        assert!(Graph::permits(
            Relationship::DeployedAs,
            &entity(EntityKind::Contract, "C-a"),
            &entity(EntityKind::Deployment, "d1")
        ));
    }

    #[test]
    fn a_self_edge_is_refused_because_a_one_node_cycle_is_not_topology() {
        let node = Node::for_entity(&entity(EntityKind::Contract, "C-a"));
        let mut edge = Edge::from_dependency(
            set_of(&[invocation("C-a", "C-b", &"a".repeat(64))]).direct[0].clone(),
        );
        // Rebuild the dependency with identical endpoints to represent the hand-built
        // case, which is the only way one can arise.
        let mut dependency = edge.dependency().clone();
        dependency.object = dependency.subject.clone();
        dependency.relationship = Relationship::DependsOn;
        edge = Edge::from_dependency(dependency);
        let graph = Graph {
            id: "g1".to_owned(),
            boundary: None,
            max_depth: None,
            truncated: false,
            truncation_reason: None,
            nodes: vec![node],
            edges: vec![edge],
            derived: Vec::new(),
            unestablished: Vec::new(),
        };
        let failures = graph.integrity_failures();
        assert!(
            failures
                .iter()
                .any(|failure| matches!(failure, GraphFailure::SelfEdge { .. })),
            "got: {failures:?}"
        );
    }

    #[test]
    fn adding_a_node_twice_is_idempotent_and_adding_a_different_one_is_refused() {
        let mut graph = Graph::new("g1").expect("a graph");
        let plain = Node::for_entity(&entity(EntityKind::Contract, "C-a"));
        graph.add_node(plain.clone()).expect("the first add");
        graph
            .add_node(plain)
            .expect("an identical add is idempotent");
        assert_eq!(graph.node_count(), 1);

        let labelled = Node::for_entity(&entity(EntityKind::Contract, "C-a")).with_label("other");
        let error = graph
            .add_node(labelled)
            .expect_err("two descriptions of one entity must not silently replace each other");
        assert!(
            error.to_string().contains("appears more than once"),
            "got: {error}"
        );
    }

    #[test]
    fn an_edge_cannot_be_added_twice_and_a_derived_dependency_is_not_an_edge() {
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        let mut graph = Graph::new("g1").expect("a graph");
        graph
            .add_edge(set.direct[0].clone())
            .expect("the first add");
        let error = graph
            .add_edge(set.direct[0].clone())
            .expect_err("one edge, one entry");
        assert!(
            error.to_string().contains("appears more than once"),
            "got: {error}"
        );

        let mut deep = set.direct[0].clone();
        deep.depth = 2;
        deep.path = vec![entity(EntityKind::Contract, "C-mid")];
        let error = graph
            .add_edge(deep)
            .expect_err("a reachability claim is not an edge");
        assert!(error.to_string().contains("depth-2"), "got: {error}");
    }

    #[test]
    fn adjacency_is_directional_and_answers_both_directions() {
        let set = set_of(&[
            invocation("C-subject", "C-a", &"a".repeat(64)),
            invocation("C-a", "C-deep", &"b".repeat(64)),
        ]);
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        let subject = entity(EntityKind::Contract, "C-subject");
        let a = entity(EntityKind::Contract, "C-a");
        let deep = entity(EntityKind::Contract, "C-deep");

        assert_eq!(graph.edges_from(&subject).len(), 1);
        assert!(graph.edges_to(&subject).is_empty());
        assert_eq!(graph.edges_to(&deep).len(), 1);
        assert_eq!(graph.neighbours(&subject), vec![&a]);
        assert_eq!(graph.edges_between(&a, &deep).len(), 1);
        assert!(
            graph.edges_between(&deep, &a).is_empty(),
            "the graph is directed"
        );
        assert_eq!(graph.roots().len(), 1);
        assert_eq!(graph.leaves().len(), 1);
    }

    #[test]
    fn a_discovered_node_with_no_edges_is_reported_rather_than_dropped() {
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        let mut graph = Graph::from_dependencies("g1", &set).expect("a graph");
        graph
            .add_entity(&entity(EntityKind::Contract, "C-unrelated"))
            .expect("a node");
        assert_eq!(graph.disconnected_nodes(), vec!["CONTRACT:C-unrelated"]);
        assert!(
            graph.validate().is_ok(),
            "an unrelated node is not a broken graph"
        );
    }

    #[test]
    fn refusals_travel_with_the_graph_so_a_report_cannot_claim_a_clean_analysis() {
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        assert!(set.unestablished.is_empty());

        let refused = Candidate::new(
            entity(EntityKind::Contract, "C-subject"),
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::DeclaredManifest,
            vec![EvidenceRef::new(EvidenceType::Source, "s1").expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());
        let with_refusal = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64)), refused]);
        let graph = Graph::from_dependencies("g1", &with_refusal).expect("a graph");
        assert_eq!(graph.unestablished.len(), 1);
        assert!(
            graph.unestablished[0].reason.contains("RUNTIME"),
            "the reason travels intact: {}",
            graph.unestablished[0].reason
        );
    }

    #[test]
    fn canonical_order_is_idempotent_and_deterministic() {
        let set = set_of(&[
            invocation("C-subject", "C-zeta", &"1".repeat(64)),
            invocation("C-subject", "C-alpha", &"2".repeat(64)),
            invocation("C-alpha", "C-deep", &"3".repeat(64)),
        ]);
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        let ids: Vec<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "nodes are ordered by kind then identifier");

        let mut again = graph.clone();
        again.canonicalise();
        assert_eq!(again, graph, "canonicalising twice changes nothing");
    }

    #[test]
    fn the_graph_is_traversable_through_the_dependency_layers_own_closure() {
        // The composition that keeps one ordering for one topology: `close` runs over the
        // graph by way of `EdgeSource`, and a graph over the same edges must reach the
        // same entities as the set it came from.
        let set = set_of(&[
            invocation("C-subject", "C-a", &"a".repeat(64)),
            invocation("C-a", "C-deep", &"b".repeat(64)),
            invocation("C-deep", "C-deeper", &"c".repeat(64)),
        ]);
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        let subject = entity(EntityKind::Contract, "C-subject");
        let closure =
            amasario_dependency::close(&subject, &graph, amasario_dependency::Limits::defaults());
        let reached: Vec<&str> = closure
            .entries
            .iter()
            .map(|entry| entry.object.id.as_str())
            .collect();
        assert!(reached.contains(&"C-deep"), "got: {reached:?}");
        assert!(reached.contains(&"C-deeper"), "got: {reached:?}");
        // Depth-one targets belong to the direct partition and must not be reported
        // again as transitive.
        assert!(!reached.contains(&"C-a"), "got: {reached:?}");
        assert!(!closure.truncated);
    }

    #[test]
    fn a_verification_status_travels_from_the_edge_to_the_graph_unchanged() {
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        assert_eq!(graph.edges[0].verification(), VerificationStatus::Verified);
        assert_eq!(graph.edges[0].basis(), Basis::ObservedInvocation);
        assert!(graph.edges[0].is_directly_observed());
    }

    #[test]
    fn a_graph_without_an_identifier_is_refused() {
        let error = Graph::new("").expect_err("a graph must be nameable");
        assert!(error.to_string().contains("identifier"), "got: {error}");
    }

    #[test]
    fn classification_still_gates_what_can_reach_a_graph() {
        let refused = Candidate::new(
            entity(EntityKind::Artifact, "artifact-1"),
            entity(EntityKind::Source, "https://example.invalid/r@abc"),
            Relationship::DependsOn,
            Basis::ConfiguredEndpoint,
            vec![EvidenceRef::new(EvidenceType::Source, "s1").expect("a citation")],
        )
        .expect("a candidate");
        assert!(
            classify(&refused).is_err(),
            "an artifact dependency cannot rest on a configured endpoint"
        );
    }

    #[test]
    fn marking_a_node_external_requires_the_node_to_exist() {
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        let mut graph = Graph::from_dependencies("g1", &set).expect("a graph");
        let outside = entity(EntityKind::Source, "https://example.invalid/r@abc");
        let error = graph
            .mark_external(&outside, true)
            .expect_err("marking an absent node would silently do nothing");
        assert!(
            error.to_string().contains("not in the graph"),
            "got: {error}"
        );

        graph
            .mark_external(&entity(EntityKind::Contract, "C-a"), true)
            .expect("an existing node");
        assert!(graph.node("CONTRACT:C-a").expect("the node").is_external());
    }

    #[test]
    fn the_network_annotation_covers_every_node_or_none_of_them() {
        let set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        let mut graph = Graph::from_dependencies("g1", &set).expect("a graph");
        assert!(!graph.nodes_are_annotated());
        graph.annotate_network();
        assert!(graph.nodes_are_annotated());
        assert_eq!(
            graph
                .attributes("CONTRACT:C-a")
                .and_then(|a| a.network_id.as_deref()),
            Some("testnet")
        );
    }

    #[test]
    fn a_graph_with_no_boundary_annotates_nothing_rather_than_guessing() {
        let graph = Graph::new("g1").expect("a graph").with_max_depth(3);
        let mut graph = graph;
        graph
            .add_entity(&entity(EntityKind::Contract, "C-a"))
            .expect("a node");
        graph.annotate_network();
        assert!(
            graph.attributes("CONTRACT:C-a").is_none(),
            "a network that was never observed must not be filled in"
        );
    }

    #[test]
    fn construction_from_a_flat_list_splits_by_depth_the_same_way() {
        let set = set_of(&[
            invocation("C-subject", "C-a", &"a".repeat(64)),
            invocation("C-a", "C-deep", &"b".repeat(64)),
        ]);
        let claim = promote_to_transitive(
            set_of(&[invocation("C-subject", "C-far", &"c".repeat(64))])
                .direct
                .remove(0),
            vec![entity(EntityKind::Contract, "C-a")],
        );

        let graph = Graph::from_dependencies_flat(
            "g1",
            vec![set.direct[0].clone(), set.direct[1].clone(), claim.clone()],
        )
        .expect("a graph");
        assert_eq!(
            graph.edge_count(),
            2,
            "only the depth-zero entries are edges"
        );
        assert_eq!(graph.derived.len(), 1);
        assert_eq!(graph.derived[0].object.id, "C-far");
        assert!(graph.validate().is_ok());

        // And the same split, reached through the set constructor, produces the same
        // topology - so a caller that already holds a set and one that holds a list
        // cannot disagree about what relates to what. The bounds and the boundary differ
        // by design: a flat list carries neither, and the set constructor records both.
        let mut via_set = set;
        via_set.transitive.push(claim);
        via_set.validate().expect("the assembled set is valid");
        let from_set = Graph::from_dependencies("g1", &via_set).expect("a graph");
        assert_eq!(from_set.nodes, graph.nodes);
        assert_eq!(from_set.edges, graph.edges);
        assert_eq!(from_set.derived, graph.derived);
        assert_eq!(from_set.max_depth, Some(5));
    }

    #[test]
    fn truncation_and_its_reason_are_set_together() {
        let graph = Graph::new("g1")
            .expect("a graph")
            .truncated_by(TruncationReason::MaxDepthReached);
        assert!(graph.truncated);
        assert_eq!(
            graph.truncation_reason,
            Some(TruncationReason::MaxDepthReached)
        );
        assert!(
            graph.validate().is_ok(),
            "the pair is internally consistent"
        );

        // The two halves cannot be broken apart, because a bounded search that does not
        // say it was bounded reads as a complete result.
        let mut unreported = graph.clone();
        unreported.truncation_reason = None;
        let error = unreported
            .validate()
            .expect_err("truncation needs a reason");
        assert!(error.to_string().contains("truncated"), "got: {error}");

        let mut unexplained = graph;
        unexplained.truncated = false;
        unexplained
            .validate()
            .expect_err("a reason without truncation misrepresents a complete result");
    }

    #[test]
    fn a_truncated_set_carries_its_reason_into_the_graph() {
        let mut set = set_of(&[invocation("C-subject", "C-a", &"a".repeat(64))]);
        set.truncated = true;
        set.truncation_reason = Some(TruncationReason::BoundaryReached);
        set.validate().expect("a complete statement validates");
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        assert!(graph.truncated);
        assert_eq!(
            graph.truncation_reason,
            Some(TruncationReason::BoundaryReached)
        );
    }
}
