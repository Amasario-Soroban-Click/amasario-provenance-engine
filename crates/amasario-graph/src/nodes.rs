//! Graph nodes: one entity per identifier, with the structural facts about it.
//!
//! # Why a node is an entity reference plus attributes, and nothing more
//!
//! `graph.schema.json` gives a node an `id`, a `kind` and a closed set of optional
//! attributes, and says of the identifier that it "must match the identifier used when
//! the entity is referenced from an edge in this graph". That sentence is the whole
//! design: a node is not a copy of an entity, it is *the* name for one, so there is
//! exactly one node per entity and an edge never has to carry a second, possibly
//! disagreeing, description of what it points at.
//!
//! # Why the identifier is `KIND:id` rather than the bare id
//!
//! [`amasario_core::EntityRef`] renders as `KIND:id` for one reason: an identifier
//! alone does not say what namespace it lives in. A digest, a contract address and a
//! repository revision can all be opaque strings, and a graph whose node identifiers
//! dropped the kind would let a `WASM` node and an `ARTIFACT` node with the same
//! content digest collapse into one - which is a real possibility, because the artifact
//! a contract was deployed from and the deployed module are content-identical and are
//! nonetheless two entities with different relationships.
//!
//! # Why the attributes are a closed set
//!
//! The schema deliberately does not accept an open bag of key/value pairs, and gives
//! its reason: "a fact that matters must be modelled, and a fact that is not modelled
//! must not be smuggled in as a string". The same rule applies here. Every field in
//! [`NodeAttributes`] corresponds to a modelled fact; there is no `extras` field,
//! because one would be a place for an unversioned, unvalidated fact to enter the
//! model through the graph.

use std::fmt;

use amasario_core::{Digest, EntityKind, EntityRef};
use serde::{Deserialize, Serialize};

/// Structural facts about a node's entity that the model gives a field to.
///
/// Every field is optional, and absence is meaningful: it means the fact was not
/// established within the observation boundary, which is a different statement from
/// the fact being false. `external`, in particular, is not "unknown": its absence is
/// "not known to be outside the boundary", and the schema says why the difference
/// matters - an entity Amasario cannot inspect must never be mistaken for one that
/// failed inspection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeAttributes {
    /// The contract address, for a `CONTRACT` node.
    #[serde(rename = "contractId", skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    /// The network the node's entity was observed on, for anything observed.
    #[serde(rename = "networkId", skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// The source revision, for a `SOURCE` or `BUILD` node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// The declared version, for a `PACKAGE` or `BUILD` node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether the node's entity lies outside the observable boundary.
    ///
    /// Recorded explicitly because the alternative is a consumer reading the absence of
    /// relationships as the absence of dependencies, when the truth is that the entity
    /// was never observable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
}

impl NodeAttributes {
    /// Whether any attribute was recorded.
    ///
    /// A node with no attributes is serialised without the field at all, which keeps a
    /// minimal graph free of empty objects that a reader would have to interpret.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.contract_id.is_none()
            && self.network_id.is_none()
            && self.revision.is_none()
            && self.version.is_none()
            && self.external.is_none()
    }

    /// Records whether the node's entity lies outside the observable boundary.
    #[must_use]
    pub const fn with_external(mut self, external: bool) -> Self {
        self.external = Some(external);
        self
    }

    /// Records the network the entity was observed on.
    #[must_use]
    pub fn with_network(mut self, network_id: impl Into<String>) -> Self {
        self.network_id = Some(network_id.into());
        self
    }

    /// Records a source revision.
    #[must_use]
    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }

    /// Records a declared version.
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

/// A typed entity in a graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// The node's identifier, which is also how an edge refers to it.
    pub id: String,
    /// What kind of entity the node is.
    pub kind: EntityKind,
    /// A human-readable label, for reports.
    ///
    /// Never used for matching. The schema is explicit about this, and the engine keeps
    /// the promise by not reading the field anywhere in the graph: a label that could
    /// influence a result would make two analyses of the same graph disagree whenever
    /// someone reworded a display string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The content digest of the node's entity, where one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    /// The closed set of structural facts about the node's entity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<NodeAttributes>,
}

impl Node {
    /// Builds the node for an entity reference.
    ///
    /// The identifier is [`EntityRef`]'s own rendering, so a node and an edge can never
    /// disagree about how an entity is named: there is one function that produces the
    /// form and both sides call it.
    #[must_use]
    pub fn for_entity(entity: &EntityRef) -> Self {
        Self {
            id: entity.to_string(),
            kind: entity.kind,
            label: None,
            digest: None,
            attributes: None,
        }
    }

    /// Builds the node for a `WASM` entity, carrying its digest.
    ///
    /// The identifier is the reference's own rendering, exactly as for any other entity:
    /// an edge names a `WASM` endpoint as `WASM:<digest>`, so a node identified by the bare
    /// digest would be a node no edge could resolve to.
    #[must_use]
    pub fn for_wasm(digest: &Digest) -> Self {
        Self {
            id: EntityRef::wasm(digest).to_string(),
            kind: EntityKind::Wasm,
            label: None,
            digest: Some(digest.clone()),
            attributes: None,
        }
    }

    /// Whether this node is the given entity.
    ///
    /// Compares the rendered identifier rather than the parts, because the identifier
    /// is what edges carry and a node that matched an entity without matching the
    /// reference form would break resolution.
    #[must_use]
    pub fn matches(&self, entity: &EntityRef) -> bool {
        self.id == entity.to_string() && self.kind == entity.kind
    }

    /// Records a human-readable label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Records the entity's content digest.
    #[must_use]
    pub fn with_digest(mut self, digest: Digest) -> Self {
        self.digest = Some(digest);
        self
    }

    /// Records structural facts about the entity.
    #[must_use]
    pub fn with_attributes(mut self, attributes: NodeAttributes) -> Self {
        self.attributes = Some(attributes);
        self
    }

    /// Whether the node is marked as lying outside the observable boundary.
    #[must_use]
    pub fn is_external(&self) -> bool {
        self.attributes
            .as_ref()
            .and_then(|attributes| attributes.external)
            .unwrap_or(false)
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)
    }
}

/// The canonical order of two nodes.
///
/// By entity kind in the vocabulary's own order, then by identifier. The kind ordering
/// is [`EntityKind::all`]'s rather than the enumeration's declaration order or the
/// alphabet, so that a graph serialised by this engine is byte-identical to one
/// serialised by any other implementation that follows the taxonomy - which is the
/// point of a controlled vocabulary.
#[must_use]
pub fn canonical_order(left: &Node, right: &Node) -> std::cmp::Ordering {
    kind_rank(left.kind)
        .cmp(&kind_rank(right.kind))
        .then_with(|| left.id.cmp(&right.id))
}

/// A node kind's position in the shared enumeration.
///
/// `usize::MAX` for a kind outside the enumeration, which cannot happen while
/// [`EntityKind`] is closed, but which orders an unknown kind last rather than
/// panicking if that ever changes.
fn kind_rank(kind: EntityKind) -> usize {
    EntityKind::all()
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{Digest, EntityKind};

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a reference")
    }

    #[test]
    fn a_node_identifier_carries_the_entity_kind() {
        // An identifier without its kind could let a WASM node and an ARTIFACT node
        // with the same content digest collapse into one.
        let node = Node::for_entity(&entity(EntityKind::Contract, "C-a"));
        assert_eq!(node.id, "CONTRACT:C-a");
        assert_eq!(node.kind, EntityKind::Contract);
        assert!(node.matches(&entity(EntityKind::Contract, "C-a")));
        assert!(
            !node.matches(&entity(EntityKind::Artifact, "C-a")),
            "the same identifier under another kind is another entity"
        );
    }

    #[test]
    fn a_wasm_node_is_identified_by_its_digest() {
        let digest = Digest::sha256_of(b"module");
        let node = Node::for_wasm(&digest);
        assert_eq!(node.kind, EntityKind::Wasm);
        assert_eq!(node.digest.as_ref(), Some(&digest));
        assert!(node.matches(&EntityRef::wasm(&digest)));
        assert_eq!(
            node.id,
            format!("WASM:{}", digest.value()),
            "an edge names a WASM endpoint as WASM:<digest>, so the node must answer to that"
        );
    }

    #[test]
    fn a_wasm_node_and_an_artifact_node_with_the_same_digest_stay_distinct() {
        // A real possibility, not a hypothetical: the artifact a contract was deployed
        // from and the deployed module are content-identical and are two entities with
        // different relationships.
        let digest = Digest::sha256_of(b"identical content");
        let wasm = Node::for_wasm(&digest);
        let artifact = Node::for_entity(
            &EntityRef::new(EntityKind::Artifact, digest.value()).expect("a reference"),
        );
        assert_ne!(wasm.id, artifact.id);
        assert_ne!(wasm, artifact);
        assert_eq!(artifact.id, format!("ARTIFACT:{}", digest.value()));
    }

    #[test]
    fn nodes_are_ordered_by_the_vocabulary_then_by_identifier() {
        let mut nodes = [
            Node::for_entity(&entity(EntityKind::Transaction, "t1")),
            Node::for_entity(&entity(EntityKind::Contract, "C-b")),
            Node::for_entity(&entity(EntityKind::Contract, "C-a")),
            Node::for_entity(&entity(EntityKind::Wasm, "d1")),
        ];
        nodes.sort_by(canonical_order);
        let ids: Vec<String> = nodes.iter().map(|node| node.id.clone()).collect();
        assert_eq!(
            ids,
            vec![
                "CONTRACT:C-a".to_owned(),
                "CONTRACT:C-b".to_owned(),
                "WASM:d1".to_owned(),
                "TRANSACTION:t1".to_owned(),
            ],
            "the vocabulary's order, not the alphabet's"
        );
        // The ordering is total, so a second sort cannot change the result.
        nodes.sort_by(canonical_order);
        let again: Vec<String> = nodes.iter().map(|node| node.id.clone()).collect();
        assert_eq!(ids, again);
    }

    #[test]
    fn an_empty_attribute_set_is_not_serialised() {
        let node = Node::for_entity(&entity(EntityKind::Contract, "C-a"));
        let json = serde_json::to_value(&node).expect("serialises");
        let object = json.as_object().expect("an object");
        assert_eq!(object.len(), 2, "only id and kind: {json}");
        assert!(!object.contains_key("attributes"));
        assert!(!object.contains_key("label"));
    }

    #[test]
    fn the_absence_of_an_attribute_is_not_the_same_as_it_being_false() {
        // `external: null` and `external: false` are different statements, and the
        // schema says why: an entity that cannot be inspected must never be confused
        // with one that was inspected and found to be inside the boundary.
        let unknown = Node::for_entity(&entity(EntityKind::Source, "https://example.invalid/r"));
        assert!(!unknown.is_external());
        assert!(unknown.attributes.is_none());

        let inside = Node::for_entity(&entity(EntityKind::Source, "https://example.invalid/r"))
            .with_attributes(NodeAttributes::default().with_external(false));
        assert!(!inside.is_external());
        assert!(inside.attributes.is_some());

        let outside = Node::for_entity(&entity(EntityKind::Source, "https://example.invalid/r"))
            .with_attributes(NodeAttributes::default().with_external(true));
        assert!(outside.is_external());
        assert_ne!(unknown, inside, "unknown and false are different nodes");
    }

    #[test]
    fn a_label_never_takes_part_in_comparison_or_ordering() {
        // A label that could influence a result would make two analyses of the same
        // graph disagree whenever a display string was reworded.
        let plain = Node::for_entity(&entity(EntityKind::Contract, "C-a"));
        let labelled = plain.clone().with_label("the treasury contract");
        assert_ne!(plain, labelled, "the label is part of the node's identity");
        assert_eq!(
            canonical_order(&plain, &labelled),
            std::cmp::Ordering::Equal
        );
        assert!(plain.matches(&entity(EntityKind::Contract, "C-a")));
        assert!(labelled.matches(&entity(EntityKind::Contract, "C-a")));
    }

    #[test]
    fn attributes_round_trip_through_their_schema_names() {
        let attributes = NodeAttributes::default()
            .with_network("testnet")
            .with_revision("1f0c4a")
            .with_version("1.2.3")
            .with_external(true);
        let json = serde_json::to_value(&attributes).expect("serialises");
        let object = json.as_object().expect("an object");
        assert_eq!(
            object.get("networkId").and_then(|v| v.as_str()),
            Some("testnet")
        );
        assert_eq!(
            object.get("revision").and_then(|v| v.as_str()),
            Some("1f0c4a")
        );
        assert_eq!(
            object.get("version").and_then(|v| v.as_str()),
            Some("1.2.3")
        );
        assert_eq!(object.get("external").and_then(|v| v.as_bool()), Some(true));

        let parsed: NodeAttributes = serde_json::from_value(json).expect("round trip");
        assert_eq!(parsed, attributes);

        // An unknown attribute is refused rather than carried, which is what "a
        // closed set" has to mean in practice.
        assert!(
            serde_json::from_str::<NodeAttributes>(r#"{"audited":true}"#).is_err(),
            "an unmodelled fact must not be smuggled in as an attribute"
        );
    }

    #[test]
    fn attribute_absence_is_reported_honestly() {
        assert!(NodeAttributes::default().is_empty());
        assert!(!NodeAttributes::default().with_external(false).is_empty());
    }
}
