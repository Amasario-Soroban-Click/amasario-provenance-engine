//! Builders for every generated fixture document.
//!
//! # Every fixture is constructed, never transcribed
//!
//! A fixture typed by hand is a document no constructor agreed to. That is survivable
//! for a plain record and impossible for a digest-bearing one: a snapshot's
//! `contentDigest` is computed over its canonical form with the declared volatile
//! fields removed, a graph edge's identifier is derived from its endpoints and its
//! relationship, and a report's canonical form is what two runs have to agree on. A
//! hand-written fixture would therefore be a document that claims a digest it does not
//! have, which is worse than no fixture at all.
//!
//! So each builder below drives the real code path - candidates through
//! [`resolve`](amasario_dependency::resolve), a set through
//! [`close_set`](amasario_dependency::close_set), a set through
//! [`Graph::from_dependencies`], findings through
//! [`analyze`](amasario_impact::analyze) - and the fixture is whatever that produced.
//! The corpus is consequently a statement about the engine rather than about the
//! corpus.
//!
//! # Determinism is a property the corpus checks
//!
//! Every builder is a pure function of constant inputs, so calling it twice produces
//! the same bytes. The suites assert that, which is how a repository full of generated
//! files stays reviewable: a diff under `fixtures/` means the model changed.

use amasario_contract::{ContractExecutableKind, ContractIdentity};
use amasario_core::{
    Basis, Confidence, ConfidenceLevel, ContractId, Digest, EntityKind, EntityRef, EvidenceType,
    LedgerSequence, Network, NetworkType, ObservationBoundary, Relationship, VerificationStatus,
};
use amasario_dependency::classifier::EvidenceRef as Citation;
use amasario_dependency::detector::detect_from_invocations;
use amasario_dependency::document::DependencySetDocument;
use amasario_dependency::transitive::{Limits, close_set};
use amasario_dependency::{DependencySet, Unestablished, resolve};
use amasario_evidence::collector::{EvidenceClass, EvidenceRecord};
use amasario_graph::{Graph, GraphDocument};
use amasario_impact::analyzer::{ImpactAnalysis, analyze, context};
use amasario_impact::change::Change;
use amasario_provenance::{
    ArtifactIdentity, ArtifactType, Attestation, BuildProvenance, ChainLink, ChainLinkKind,
    DeploymentKind, DeploymentProvenance, ProvenanceChain, Repository, Reproducibility, Revision,
    SignatureState, SourceProvenance, Toolchain, VcsKind,
};
use amasario_report::model::{
    ConfidenceEntry, InferredStatement, Report, Statement, UnknownEntry, UnknownReason,
    VerificationEntry,
};
use amasario_snapshot::Capture;

use crate::wasm_modules;

/// The passphrase of the chain every fixture is observed on.
///
/// Stellar's published testnet passphrase. It is a protocol fact rather than a
/// configuration value, which is why it is written out here rather than invented.
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// The repository the engine's own source is at, used as the subject of the source
/// records so that the provenance fixtures describe something real.
pub const REPOSITORY_URL: &str =
    "https://github.com/Amasario-Soroban-Click/amasario-provenance-engine";

/// The commit the fixtures' build records claim.
pub const REVISION_COMMIT: &str = "3f2c1a0d9e8b7c6f5a4b3c2d1e0f9a8b7c6d5e4f";

/// A transaction hash. Not a real transaction, and never presented as one: it is the
/// citation a fixture's evidence record carries.
pub const DEPLOY_TRANSACTION: &str =
    "9f2a7c1e4b8d3f6a0c5e2b9d7f4a1c8e6b3d0f9a2c7e4b1d8f5a2c9e6b3d0f7a";

/// The observed-at timestamp every fixture shares.
pub const OBSERVED_AT: &str = "2026-01-01T00:00:00Z";

/// The ledger every fixture is bounded at.
pub const BOUNDARY_LEDGER: u32 = 1_044;

/// Builds a contract address from one repeated byte.
///
/// The strkey checksum is computed rather than transcribed, so the addresses are
/// genuinely valid and the engine's own checksum verification is exercised on them
/// rather than bypassed.
///
/// # Panics
///
/// Panics when the payload does not encode, which cannot happen for 32 bytes.
#[must_use]
pub fn contract_address(seed: u8) -> ContractId {
    let payload = [seed; 32];
    let strkey = stellar_strkey::Contract(payload).to_string();
    ContractId::new(strkey.as_str()).expect("a freshly encoded contract address verifies")
}

/// The subject of every fixture: the contract whose dependencies are analysed.
#[must_use]
pub fn alpha() -> EntityRef {
    entity(0xa1)
}

/// A contract `alpha` invokes.
#[must_use]
pub fn bravo() -> EntityRef {
    entity(0xb2)
}

/// A contract `bravo` invokes.
#[must_use]
pub fn charlie() -> EntityRef {
    entity(0xc3)
}

/// The contract at the end of the deepest chain.
#[must_use]
pub fn delta() -> EntityRef {
    entity(0xd4)
}

/// A contract no fixture relates to anything, so that the corpus holds a node that is
/// disconnected rather than every node having a place in a chain.
#[must_use]
pub fn echo() -> EntityRef {
    entity(0xe5)
}

fn entity(seed: u8) -> EntityRef {
    EntityRef::new(EntityKind::Contract, contract_address(seed).as_str())
        .expect("a contract reference")
}

/// A package entity, which is what a declared dependency points at.
///
/// # Panics
///
/// Panics never; the identifier is a constant.
#[must_use]
pub fn package(name: &str) -> EntityRef {
    EntityRef::new(EntityKind::Package, name).expect("a package reference")
}

/// The testnet network descriptor.
///
/// # Panics
///
/// Panics never; the two constants are well formed.
#[must_use]
pub fn network() -> Network {
    Network::new("testnet", NetworkType::Testnet, TESTNET_PASSPHRASE).expect("a network")
}

/// The observation boundary every fixture shares.
///
/// # Panics
///
/// Panics never; the ledger is in range.
#[must_use]
pub fn boundary() -> ObservationBoundary {
    ObservationBoundary::new(
        network(),
        LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"),
        OBSERVED_AT,
    )
}

/// A digest from a repeated byte, so that fixtures do not share a digest by accident.
///
/// # Panics
///
/// Panics never; 64 hex characters is a well-formed SHA-256 value.
#[must_use]
pub fn digest_of(byte: u8) -> Digest {
    let hex = format!("{byte:02x}").repeat(32);
    Digest::new(amasario_core::DigestAlgorithm::Sha256, &hex).expect("a well-formed digest")
}

/// The digest of one of the corpus's real WebAssembly modules.
///
/// # Panics
///
/// Panics when the module name is not one of [`wasm_modules::all`].
#[must_use]
pub fn module_digest(name: &str) -> Digest {
    let spec = wasm_modules::all()
        .into_iter()
        .find(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not a corpus module"));
    amasario_contract::wasm::digest_of(&(spec.bytes)())
}

/// A recorded transaction hash.
///
/// # Panics
///
/// Panics never; the constant is 64 hexadecimal characters.
#[must_use]
pub fn deploy_transaction() -> amasario_core::TransactionHash {
    amasario_core::TransactionHash::new(DEPLOY_TRANSACTION).expect("a transaction hash")
}

/// One observed cross-contract call, as the inspection layer reports them.
#[derive(Debug, Clone, Copy)]
pub struct Call {
    /// The contract that was entered.
    pub callee: fn() -> EntityRef,
    /// The contract that entered it; `None` for a call made by a transaction.
    pub caller: Option<fn() -> EntityRef>,
    /// The function entered.
    pub function: &'static str,
    /// Whether the ledger recorded the call as succeeding. `None` when the endpoint
    /// did not say, which is not the same as `Some(false)`.
    pub successful: Option<bool>,
    /// Whether the call was recovered from a diagnostic event rather than an
    /// operation, which is the difference between the two observed bases.
    pub from_event: bool,
    /// A transaction hash distinguishing this call's citation.
    pub transaction_seed: u8,
}

impl Call {
    fn invocation(&self) -> amasario_contract::ContractInvocation {
        let seed = self.transaction_seed;
        let hex = format!("{seed:02x}").repeat(32);
        amasario_contract::ContractInvocation {
            callee: (self.callee)().id,
            caller: self.caller.map(|caller| caller().id),
            function: Some(self.function.to_owned()),
            transaction: amasario_core::TransactionHash::new(hex)
                .expect("a transaction hash built from repeated bytes"),
            ledger: Some(LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger")),
            operation_index: Some(1),
            basis: if self.from_event {
                Basis::ObservedEvent
            } else {
                Basis::ObservedInvocation
            },
            event_id: self.from_event.then(|| format!("event-{seed:02x}-1")),
            successful: self.successful,
        }
    }
}

/// A call `alpha` makes to `bravo`, recorded as succeeding.
pub const CALL_BRAVO: Call = Call {
    callee: bravo,
    caller: Some(alpha),
    function: "transfer",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x01,
};

/// A call `alpha` makes to `charlie`, recorded as succeeding.
pub const CALL_CHARLIE: Call = Call {
    callee: charlie,
    caller: Some(alpha),
    function: "balance",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x02,
};

/// A call `bravo` makes to `charlie`. `bravo` is the subject of the transitive
/// fixture's second hop.
pub const CALL_CHARLIE_FROM_BRAVO: Call = Call {
    callee: charlie,
    caller: Some(bravo),
    function: "balance",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x03,
};

/// A call `charlie` makes to `delta`, the deepest hop the corpus records.
pub const CALL_DELTA_FROM_CHARLIE: Call = Call {
    callee: delta,
    caller: Some(charlie),
    function: "settle",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x04,
};

/// A call `charlie` makes back to `bravo`, which closes a cycle.
///
/// The call is real, so the cycle is a fact about the graph rather than a defect in
/// the fixture: it is what a mutually recursive pair of contracts looks like.
pub const CALL_BRAVO_FROM_CHARLIE: Call = Call {
    callee: bravo,
    caller: Some(charlie),
    function: "transfer",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x05,
};

/// A call the endpoint never said the outcome of.
///
/// This is the refusal the corpus exists to hold: a `RUNTIME` dependency must rest on
/// a transaction recorded as successful, and an unknown outcome is not a successful
/// one. The candidate is refused rather than silently promoted.
pub const CALL_OUTCOME_UNKNOWN: Call = Call {
    callee: echo,
    caller: Some(alpha),
    function: "mint",
    successful: None,
    from_event: false,
    transaction_seed: 0x06,
};

/// A call `alpha` makes to itself.
pub const CALL_SELF: Call = Call {
    callee: alpha,
    caller: Some(alpha),
    function: "recur",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x07,
};

/// A call made by a transaction rather than by a contract.
pub const CALL_TOP_LEVEL: Call = Call {
    callee: charlie,
    caller: None,
    function: "balance",
    successful: Some(true),
    from_event: false,
    transaction_seed: 0x08,
};

/// A call recovered from a diagnostic event instead of from an operation.
pub const CALL_FROM_EVENT: Call = Call {
    callee: delta,
    caller: Some(alpha),
    function: "settle",
    successful: Some(true),
    from_event: true,
    transaction_seed: 0x09,
};

/// Resolves a set of recorded calls into a validated dependency set.
///
/// # Panics
///
/// Panics when the engine refuses to build the set, because a corpus that held a set
/// the engine would not construct would be a fixture describing a state the engine
/// cannot reach.
#[must_use]
pub fn dependency_set(subject: EntityRef, calls: &[Call], max_depth: usize) -> DependencySet {
    let boundary = boundary();
    let invocations: Vec<_> = calls.iter().map(Call::invocation).collect();
    let report = detect_from_invocations(&subject, &boundary, &invocations)
        .unwrap_or_else(|error| panic!("the recorded calls are usable: {error}"));
    resolve(subject, Some(boundary), &report.candidates, max_depth)
        .unwrap_or_else(|error| panic!("the resolved set satisfies its rules: {error}"))
}

/// Closes a set over the graph its own edges describe, which is what a transitive
/// fixture needs and what a cycle is discovered by.
///
/// # Panics
///
/// Panics when the closure is refused, which would mean the corpus held edges the
/// traversal cannot relate.
#[must_use]
pub fn closed(subject: EntityRef, calls: &[Call], max_depth: usize) -> DependencySet {
    let mut set = dependency_set(subject, calls, max_depth);
    let edges = observed_edges();
    let limits = Limits::new(
        max_depth,
        amasario_dependency::transitive::DEFAULT_MAX_NODES,
    )
    .expect("non-zero bounds");
    // The closure source is the union of every observer's edges rather than the set
    // itself. A dependency set holds only the *subject's* observations, because a call
    // made by another contract belongs to that contract's set - so closing over the
    // set alone would find no second hop. Walking the union is what the engine's
    // pipeline does, and it is what makes a transitive fixture transitive.
    close_set(&mut set, &edges, limits)
        .unwrap_or_else(|error| panic!("the closure satisfies its rules: {error}"));
    set
}

/// The calls each observer in the corpus was seen to make.
///
/// Grouped by observer because the detector attributes a call to the contract that made
/// it: an observation whose caller is not the subject is skipped as belonging to another
/// contract's set.
/// The subject's own calls are deliberately the narrowest set in the corpus.
///
/// A transitive dependency is one the subject reaches *only* through an intermediate, and
/// the traversal skips a target it has already reached at a shorter distance. If the
/// subject also invoked the far contract directly - which an earlier version of this
/// corpus had it do - then the second hop is never the route that establishes the
/// dependency, and the transitive fixture silently becomes a direct one. The corpus is
/// arranged so that the far contract is reachable only through `bravo`.
const OBSERVED_CALLS: &[(fn() -> EntityRef, &[Call])] = &[
    (alpha, &[CALL_BRAVO]),
    (bravo, &[CALL_CHARLIE_FROM_BRAVO]),
    (charlie, &[CALL_DELTA_FROM_CHARLIE]),
];

/// The call that closes the corpus's chain back on itself.
///
/// Kept out of [`OBSERVED_CALLS`] on purpose. The chain is walked by every closure in
/// the corpus, and a closure that reached `charlie` would find this edge and report a
/// cycle in what is meant to be an acyclic fixture. The `cyclic` graph adds it back
/// explicitly, so the cycle is a property of the fixture that is *about* a cycle rather
/// than something every fixture inherits.
///
/// The call is real, so the cycle is a fact about the graph rather than a defect in the
/// fixture: it is what a mutually recursive pair of contracts looks like.
const CLOSING_CALLS: &[(fn() -> EntityRef, &[Call])] = &[(charlie, &[CALL_BRAVO_FROM_CHARLIE])];

/// Every edge any observer in the corpus was seen to establish.
///
/// # Panics
///
/// Panics when a set cannot be resolved, which would mean the corpus held observations
/// the resolver refuses rather than records.
#[must_use]
pub fn observed_edges() -> Vec<amasario_dependency::Dependency> {
    collect_edges(OBSERVED_CALLS)
}

/// The edge that closes the corpus's chain back on itself.
///
/// Separate from [`observed_edges`] because only the `cyclic` graph holds it. See
/// [`CLOSING_CALLS`] for why the ordinary chain must not.
///
/// # Panics
///
/// Panics when the set cannot be resolved, which would mean the corpus held an
/// observation the resolver refuses rather than a record of one.
#[must_use]
pub fn closing_edges() -> Vec<amasario_dependency::Dependency> {
    collect_edges(CLOSING_CALLS)
}

/// Every edge the observers in one table were seen to establish, in canonical order.
///
/// # Panics
///
/// Panics when a set cannot be resolved, which would mean the corpus held observations
/// the resolver refuses rather than records.
fn collect_edges(
    observers: &[(fn() -> EntityRef, &[Call])],
) -> Vec<amasario_dependency::Dependency> {
    let mut edges = Vec::new();
    for (subject, calls) in observers {
        let set = dependency_set(subject(), calls, 1);
        edges.extend(set.direct);
    }
    // Canonical order, so that two runs produce one closure and one document.
    edges.sort_by(|left, right| {
        left.subject
            .id
            .cmp(&right.subject.id)
            .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
            .then_with(|| left.object.id.cmp(&right.object.id))
    });
    edges
}

/// The dependency sets the corpus holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetKind {
    /// `alpha` invokes `bravo` and `charlie`, one edge each.
    Direct,
    /// `alpha` invokes `bravo`, and `bravo` invokes `charlie`.
    Transitive,
    /// A direct edge, a two-hop edge, a refused candidate and a skipped observation.
    Mixed,
    /// Nothing resolved, and why: a call whose outcome the endpoint did not report, a
    /// self-call, and a call made by a transaction rather than by a contract.
    Refused,
}

impl SetKind {
    /// Every kind, in the order the generator writes them.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Direct, Self::Transitive, Self::Mixed, Self::Refused]
    }

    /// The fixture file name, without the extension.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Transitive => "transitive",
            Self::Mixed => "mixed",
            Self::Refused => "refused",
        }
    }
}

/// Builds one dependency set.
#[must_use]
pub fn set(kind: SetKind) -> DependencySet {
    match kind {
        SetKind::Direct => dependency_set(alpha(), &[CALL_BRAVO, CALL_CHARLIE], 1),
        SetKind::Transitive => closed(alpha(), &[CALL_BRAVO], 2),
        SetKind::Mixed => {
            // Three direct edges - one from an operation, one from an event and one from
            // an operation the ledger reported as failing - alongside one refused
            // candidate. No closure: this subject reaches everything it reaches
            // directly, and a closure over the corpus's union would report a second hop
            // that the set already holds as a direct edge, which the partition rule
            // forbids.
            let mut set = dependency_set(alpha(), &[CALL_BRAVO, CALL_CHARLIE, CALL_FROM_EVENT], 3);
            // The refused candidate and the skipped observation are part of what the
            // engine reported, so the fixture carries them rather than only the edges
            // that happened to resolve.
            set.unestablished.push(Unestablished {
                object: echo(),
                relationship: Relationship::Invocates,
                basis: Basis::ObservedInvocation,
                reason: "the endpoint did not report whether the invocation succeeded, and \
                         an unknown outcome is not a successful one"
                    .to_owned(),
                detail: Some("alpha called mint on the contract at 0xe5".to_owned()),
            });
            set.unestablished.sort_by(|left, right| {
                left.object
                    .id
                    .cmp(&right.object.id)
                    .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
            });
            set.validate()
                .expect("the set with a refusal still satisfies its rules");
            set
        },
        SetKind::Refused => {
            // The call's outcome was not reported, so no class can be established for
            // it; the set records the refusal with the reasoning rather than the edge.
            let set = dependency_set(alpha(), &[CALL_OUTCOME_UNKNOWN], 1);
            set.validate().expect("an empty set is a valid set");
            set
        },
    }
}

/// The dependency set document for one kind, in the specification's shape.
///
/// # Panics
///
/// Panics when the projection is refused, which would mean the document the
/// specification defines cannot be built from the set the engine produced.
#[must_use]
pub fn set_document(kind: SetKind) -> DependencySetDocument {
    DependencySetDocument::of(&set(kind)).unwrap_or_else(|error| {
        panic!("the set projects onto the specification's document: {error}")
    })
}

/// The graphs the corpus holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphKind {
    /// Two edges from one subject.
    Direct,
    /// A two-hop chain.
    Transitive,
    /// A four-entity chain.
    MultiHop,
    /// A chain that closes back on itself.
    Cyclic,
    /// A graph with a contract no edge reaches.
    Disconnected,
}

impl GraphKind {
    /// Every kind, in the order the generator writes them.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Direct,
            Self::Transitive,
            Self::MultiHop,
            Self::Cyclic,
            Self::Disconnected,
        ]
    }

    /// The fixture file name, without the extension.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Transitive => "transitive",
            Self::MultiHop => "multi-hop",
            Self::Cyclic => "cyclic",
            Self::Disconnected => "disconnected",
        }
    }
}

/// Builds one graph.
///
/// # Panics
///
/// Panics when the edges do not form a graph, which would mean the relationship
/// vocabulary refused a pair the dependency layer established.
#[must_use]
pub fn graph(kind: GraphKind) -> Graph {
    // A graph is assembled edge by edge rather than from a set, because a set holds only
    // one subject's observations: a cycle is formed by two contracts' edges, and a graph
    // built from one subject's set could never contain one. The `Cyclic` kind therefore
    // uses the whole corpus's union, which is what a real pipeline does when it resolves
    // every contract it discovered and assembles one graph from the result.
    let (nodes, edges) = match kind {
        GraphKind::Direct => edges_of(set(SetKind::Direct), &observed_edges()),
        GraphKind::Transitive => edges_of(set(SetKind::Transitive), &observed_edges()),
        GraphKind::MultiHop => edges_of(closed(alpha(), &[CALL_BRAVO], 3), &observed_edges()),
        GraphKind::Cyclic => {
            // The acyclic chain plus the edge that closes it. The closure functions are
            // never given this edge, so `transitive` and `multi-hop` stay acyclic while
            // this fixture holds a cycle the traversal can actually find.
            let mut edges = observed_edges();
            edges.extend(closing_edges());
            let nodes = nodes_of(&edges, &[]);
            (nodes, edges)
        },
        GraphKind::Disconnected => {
            // `echo` is unrelated to the subject, which is what a disconnected node is.
            // Adding it explicitly is the point: a graph whose every node lies on a path
            // would never exercise the traversal's disconnected case.
            let (mut nodes, edges) = edges_of(dependency_set(alpha(), &[CALL_BRAVO], 1), &[]);
            nodes.push(echo());
            (nodes, edges)
        },
    };

    let mut graph = Graph::new(format!("amasario.{}", kind.file()))
        .unwrap_or_else(|error| panic!("the graph identifier is usable: {error}"));
    for entity in &nodes {
        graph
            .add_entity(entity)
            .unwrap_or_else(|error| panic!("a node can be added: {error}"));
    }
    for dependency in &edges {
        graph
            .add_edge(dependency.clone())
            .unwrap_or_else(|error| panic!("the edge is permitted: {error}"));
    }
    graph = graph.with_boundary(boundary());
    graph.canonicalise();
    graph
        .validate()
        .unwrap_or_else(|error| panic!("the graph is valid: {error}"));
    graph
}

/// The distinct nodes and every *direct* edge a set establishes.
///
/// A graph holds direct edges only: [`amasario_graph::Graph::add_edge`] refuses a
/// transitive one, because a transitive dependency is a whole path rather than a single
/// hop and publishing it as an edge would put one relationship in two layers with two
/// different meanings. A transitive entry is therefore decomposed into the hops it was
/// established through, looked up among the direct edges the corpus observed - so the
/// graph holds the route the traversal actually walked rather than a leap over it.
///
/// The path's intermediates are nodes as well as the endpoints: a transitive edge was
/// established through them, and a graph that omitted them would hold an edge whose
/// route cannot be walked.
fn edges_of(
    set: DependencySet,
    source: &[amasario_dependency::Dependency],
) -> (Vec<EntityRef>, Vec<amasario_dependency::Dependency>) {
    let mut nodes: Vec<EntityRef> = Vec::new();
    let mut chains: Vec<Vec<EntityRef>> = Vec::new();
    let mut edges: Vec<amasario_dependency::Dependency> = Vec::new();
    for dependency in set.all() {
        for entity in std::iter::once(&dependency.subject)
            .chain(std::iter::once(&dependency.object))
            .chain(dependency.path.iter())
        {
            if !nodes.contains(entity) {
                nodes.push(entity.clone());
            }
        }
        if dependency.depth == 0 {
            push_edge(&mut edges, dependency.clone());
        } else {
            chains.push(
                std::iter::once(dependency.subject.clone())
                    .chain(dependency.path.iter().cloned())
                    .chain(std::iter::once(dependency.object.clone()))
                    .collect(),
            );
        }
    }
    for chain in chains {
        for hop in chain.windows(2) {
            let (from, to) = (&hop[0], &hop[1]);
            let edge = source
                .iter()
                .find(|candidate| {
                    candidate.subject == *from && candidate.object == *to && candidate.depth == 0
                })
                .unwrap_or_else(|| {
                    panic!(
                        "a transitive path from {} to {} traverses {} -> {}, which is not \
                         among the observed direct edges; a graph cannot hold a hop that \
                         was never observed",
                        chain.first().expect("a chain has a start").id,
                        chain.last().expect("a chain has an end").id,
                        from.id,
                        to.id
                    )
                });
            push_edge(&mut edges, edge.clone());
        }
    }
    (nodes, edges)
}

/// Every entity named by an edge, in first-seen order.
fn nodes_of(edges: &[amasario_dependency::Dependency], extra: &[EntityRef]) -> Vec<EntityRef> {
    let mut nodes: Vec<EntityRef> = extra.to_vec();
    for dependency in edges {
        for entity in std::iter::once(&dependency.subject)
            .chain(std::iter::once(&dependency.object))
            .chain(dependency.path.iter())
        {
            if !nodes.contains(entity) {
                nodes.push(entity.clone());
            }
        }
    }
    nodes
}

/// Adds an edge unless an edge with the same endpoints and relationship is already held.
///
/// Deduplication is by the triple the graph derives an edge's identifier from, so what
/// is dropped here is exactly what [`amasario_graph::Graph::add_edge`] would refuse.
fn push_edge(
    edges: &mut Vec<amasario_dependency::Dependency>,
    edge: amasario_dependency::Dependency,
) {
    let duplicate = edges.iter().any(|existing| {
        existing.subject == edge.subject
            && existing.object == edge.object
            && existing.relationship == edge.relationship
    });
    if !duplicate {
        edges.push(edge);
    }
}

/// The graph document for one kind.
#[must_use]
pub fn graph_document(kind: GraphKind) -> GraphDocument {
    GraphDocument::of(&graph(kind))
}

/// The provenance chains the corpus holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceKind {
    /// Source, revision, build, artifact, WASM and deployment all established.
    Verified,
    /// The source and build are known; the deployment transaction is not.
    Partial,
    /// Nothing established but the contract's existence.
    UnknownSource,
    /// A chain whose deployment records a module that disagrees with the built one.
    MismatchedWasm,
}

impl ProvenanceKind {
    /// Every kind, in the order the generator writes them.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Verified,
            Self::Partial,
            Self::UnknownSource,
            Self::MismatchedWasm,
        ]
    }

    /// The fixture file name, without the extension.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Partial => "partial",
            Self::UnknownSource => "unknown-source",
            Self::MismatchedWasm => "mismatched-wasm",
        }
    }

    /// The links this chain holds, in the order the model requires.
    ///
    /// Stated as data rather than as repetition, and constant so that a chain's shape
    /// is visible in one place: each kind differs in how far the evidence reaches and
    /// in what the reachable links are worth, and a reader comparing two fixtures can
    /// see the difference in the table rather than by diffing two blocks of builder
    /// calls.
    #[must_use]
    pub fn links(self) -> Vec<(ChainLinkKind, VerificationStatus, Vec<String>)> {
        use ChainLinkKind::{
            ArtifactToWasm, BuildToArtifact, DeploymentToContract, SourceResolved, SourceToBuild,
            WasmToDeployment,
        };
        use VerificationStatus::{Conflicting, PartiallyVerified, Unverified, Verified};

        let owned = |values: &[&str]| -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        };

        match self {
            Self::Verified => vec![
                (SourceResolved, Verified, owned(&["e-source-1"])),
                (SourceToBuild, Verified, owned(&["e-build-1"])),
                (BuildToArtifact, Verified, owned(&["e-build-1"])),
                (ArtifactToWasm, Verified, owned(&["e-artifact-1"])),
                (WasmToDeployment, Verified, owned(&["e-deployment-1"])),
                (DeploymentToContract, Verified, owned(&["e-deployment-1"])),
            ],
            Self::Partial => vec![
                (SourceResolved, Verified, owned(&["e-source-1"])),
                (SourceToBuild, Verified, owned(&["e-build-1"])),
                (BuildToArtifact, PartiallyVerified, owned(&["e-build-1"])),
                (ArtifactToWasm, PartiallyVerified, owned(&["e-artifact-1"])),
                (WasmToDeployment, Unverified, owned(&["e-deployment-1"])),
            ],
            Self::UnknownSource => vec![(
                DeploymentToContract,
                Unverified,
                owned(&["e-observation-1"]),
            )],
            Self::MismatchedWasm => vec![
                (SourceResolved, Verified, owned(&["e-source-1"])),
                (SourceToBuild, Verified, owned(&["e-build-1"])),
                (BuildToArtifact, Verified, owned(&["e-build-1"])),
                (
                    ArtifactToWasm,
                    Conflicting,
                    owned(&["e-artifact-1", "e-deployment-1"]),
                ),
                (
                    WasmToDeployment,
                    Conflicting,
                    owned(&["e-artifact-1", "e-deployment-1"]),
                ),
                (DeploymentToContract, Verified, owned(&["e-deployment-1"])),
            ],
        }
    }
}

/// The source record every provenance fixture shares.
///
/// # Panics
///
/// Panics when a constructor refuses the constants, which cannot happen.
#[must_use]
pub fn source() -> SourceProvenance {
    let repository =
        Repository::new(REPOSITORY_URL, VcsKind::Git).expect("the repository URL is absolute");
    let revision = Revision::commit(REVISION_COMMIT).expect("a commit identifier");
    SourceProvenance::new(repository, revision, vec!["e-source-1".to_owned()])
        .expect("a source claim cites its evidence")
        .in_subdirectory("crates/amasario-cli")
        .retrieved_at(OBSERVED_AT)
}

/// The build record every provenance fixture shares.
///
/// # Panics
///
/// Panics when a constructor refuses the constants, which cannot happen.
#[must_use]
pub fn build() -> BuildProvenance {
    let toolchain = Toolchain::new("rustc", "1.93.0")
        .expect("a toolchain identity")
        .with_source("https://static.rust-lang.org");
    let artifact = ArtifactIdentity::new(
        module_digest("custom-section"),
        ArtifactType::BuildArtifact,
        vec!["e-build-1".to_owned()],
    )
    .expect("a build artifact cites its evidence")
    .with_size(1_024)
    .with_file_name("amasario_cli.wasm");

    BuildProvenance::new(
        Revision::commit(REVISION_COMMIT).expect("a commit identifier"),
        toolchain,
        "wasm32-unknown-unknown",
        artifact,
        vec!["e-build-1".to_owned()],
    )
    .expect("a build claim cites its evidence")
    .with_configuration_digest(digest_of(0x11))
    .with_lockfile_digest(digest_of(0x22))
    .with_reproducibility(
        Reproducibility::reproduced(vec!["e-build-1".to_owned()], None)
            .expect("a reproducibility claim cites its evidence"),
    )
}

/// The deployment record every provenance fixture shares.
///
/// # Panics
///
/// Panics when a constructor refuses the constants.
#[must_use]
pub fn deployment(wasm_hash: Digest, transaction: bool) -> DeploymentProvenance {
    let mut record = DeploymentProvenance::new(
        contract_address(0xa1),
        "testnet",
        TESTNET_PASSPHRASE,
        LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"),
        DeploymentKind::Deploy,
        vec!["e-deployment-1".to_owned()],
    )
    .expect("a deployment claim cites its evidence")
    .with_wasm_hash(wasm_hash)
    .expect("the module digest is well formed");
    if transaction {
        // The operation's position is not a separate field on the record: the
        // transaction and its index are one observation, and the deployment provenance
        // type carries them together so that a citation cannot name a transaction
        // without saying where in it the deployment was.
        record.transaction = Some(deploy_transaction());
        record.operation_index = Some(1);
    }
    record
}

/// Builds one provenance chain.
///
/// # Panics
///
/// Panics when a link is refused, which would mean the chain described a sequence the
/// model does not permit.
#[must_use]
pub fn provenance_chain(kind: ProvenanceKind) -> ProvenanceChain {
    let contract = alpha();
    let mut chain = ProvenanceChain::new(contract.clone()).expect("a contract chain");

    let entities = ChainEntities::new(contract);

    for (link_kind, verification, citations) in kind.links() {
        let link = entities.link(link_kind, verification, citations);
        chain
            .push(link)
            .unwrap_or_else(|error| panic!("links are pushed in order: {error}"));
    }

    chain
}

/// The entities a provenance chain relates, in one place.
///
/// A chain link's *subject* is the downstream entity and its *object* the upstream
/// one - `SOURCE_TO_BUILD` connects a `BUILD` to the `SOURCE` it read - so the
/// direction is the model's and not a choice made here. Naming each entity once keeps
/// every link from having to restate it, and keeps a link that had them the wrong way
/// round from compiling into a chain that looks plausible and means the opposite.
struct ChainEntities {
    source: EntityRef,
    build: EntityRef,
    artifact: EntityRef,
    wasm: EntityRef,
    deployment: EntityRef,
    contract: EntityRef,
}

impl ChainEntities {
    fn new(contract: EntityRef) -> Self {
        Self {
            source: EntityRef::new(EntityKind::Source, "amasario-provenance-engine")
                .expect("a source reference"),
            build: EntityRef::new(EntityKind::Build, "amasario-cli-1.0.0")
                .expect("a build reference"),
            artifact: EntityRef::new(EntityKind::Artifact, "amasario_cli.wasm")
                .expect("an artifact reference"),
            wasm: EntityRef::wasm(&module_digest("custom-section")),
            deployment: EntityRef::new(
                EntityKind::Deployment,
                format!("{}@{BOUNDARY_LEDGER}", contract_address(0xa1).as_str()),
            )
            .expect("a deployment reference"),
            contract,
        }
    }

    fn link(
        &self,
        kind: ChainLinkKind,
        verification: VerificationStatus,
        citations: Vec<String>,
    ) -> ChainLink {
        let (subject, object) = match kind {
            ChainLinkKind::SourceResolved => (self.source.clone(), None),
            ChainLinkKind::SourceToBuild => (self.build.clone(), Some(self.source.clone())),
            ChainLinkKind::BuildToArtifact => (self.artifact.clone(), Some(self.build.clone())),
            ChainLinkKind::ArtifactToWasm => (self.wasm.clone(), Some(self.artifact.clone())),
            ChainLinkKind::WasmToDeployment => (self.wasm.clone(), Some(self.deployment.clone())),
            ChainLinkKind::DeploymentToContract => {
                (self.contract.clone(), Some(self.deployment.clone()))
            },
            // The vocabulary is non-exhaustive, so a future link kind must not be
            // silently given the wrong endpoints. This is unreachable for every kind
            // the current model names, and loud rather than wrong if that changes.
            _ => panic!(
                "the corpus has no entities for the chain link kind {kind}, so a fixture \
                 cannot be built for it; add it to ChainEntities"
            ),
        };

        // The level is capped by the basis, not copied from the verification status. The
        // two are independent: `Verified` says the evidence establishes the link, while
        // the confidence says how strong that evidence is. Nothing in a provenance chain
        // can reach `VERIFIED` confidence, because that level is reserved for a
        // dependency established by an *observed* invocation or event - the strongest
        // basis the vocabulary has - and no build, artifact or deployment relationship
        // can be observed that way. Claiming otherwise would be exactly the
        // manufactured certainty the specification forbids.
        let level = match verification {
            VerificationStatus::Verified => ConfidenceLevel::HighConfidence,
            VerificationStatus::PartiallyVerified => ConfidenceLevel::MediumConfidence,
            VerificationStatus::Conflicting => ConfidenceLevel::LowConfidence,
            _ => ConfidenceLevel::Unknown,
        };
        let confidence = Confidence::new(level, citations, Vec::new())
            .unwrap_or_else(|error| panic!("a confidence with a citation: {error}"));

        // `EMBEDDED_DIGEST` is the basis for every link here: each one is established by
        // comparing a recorded digest against a computed one, which is what the corpus
        // can actually demonstrate. Claiming `ATTESTED` would assert an issuer the
        // corpus does not have.
        ChainLink::new(
            kind,
            subject,
            object,
            Basis::EmbeddedDigest,
            confidence,
            verification,
        )
        .unwrap_or_else(|error| panic!("the link is permitted: {error}"))
    }
}

/// The impact analyses the corpus holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpactKind {
    /// One hop from the change.
    Direct,
    /// Two hops, starting at a dependency rather than at the subject.
    Transitive,
    /// A four-entity chain, reached to its end.
    MultiHop,
    /// The same chain under a bound that stops before its end, so that the analysis has
    /// to disclose that it was bounded.
    Bounded,
}

impl ImpactKind {
    /// Every kind, in the order the generator writes them.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Direct,
            Self::Transitive,
            Self::MultiHop,
            Self::Bounded,
        ]
    }

    /// The fixture file name, without the extension.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Transitive => "transitive",
            Self::MultiHop => "multi-hop",
            Self::Bounded => "bounded",
        }
    }

    const fn graph_kind(self) -> GraphKind {
        match self {
            Self::Direct => GraphKind::Direct,
            Self::Transitive => GraphKind::Transitive,
            Self::MultiHop | Self::Bounded => GraphKind::MultiHop,
        }
    }

    /// The entity the change starts at.
    ///
    /// Always the *object* end of a chain, never the subject. A relationship declares
    /// its own propagation direction, and every relationship this corpus uses - the
    /// `INVOCATES` edge a cross-contract call establishes - propagates from its object
    /// to its subject, because a change to a callee is what reaches its callers. A
    /// change to a caller does not travel to what it calls, so naming the subject here
    /// would produce an analysis that finds nothing, which is the failure the `direct`
    /// fixture exists to catch.
    #[must_use]
    pub fn changed(self) -> EntityRef {
        match self {
            // One hop from the change: `alpha` invokes `bravo`.
            Self::Direct => bravo(),
            // A contract the subject requires, two hops away.
            Self::Transitive => charlie(),
            // The far end of the chain, so the analysis has to walk it to reach `alpha`.
            Self::MultiHop | Self::Bounded => delta(),
        }
    }

    const fn depth(self) -> usize {
        match self {
            Self::Direct => 1,
            Self::Transitive => 2,
            Self::MultiHop => 4,
            // The same graph as `MultiHop` under a bound that cannot reach its end.
            Self::Bounded => 2,
        }
    }

    /// The change type, which every finding carries.
    const fn change_type(self) -> amasario_impact::ChangeType {
        amasario_impact::ChangeType::Modified
    }
}

/// Builds one impact analysis.
///
/// # Panics
///
/// Panics when the analysis is refused, which would mean a finding failed a rule the
/// derivation was supposed to satisfy by construction.
#[must_use]
pub fn impact_analysis(kind: ImpactKind) -> ImpactAnalysis {
    let graph = graph(kind.graph_kind());
    let context = context(&graph, &[]);
    let limits = Limits::new(
        kind.depth(),
        amasario_dependency::transitive::DEFAULT_MAX_NODES,
    )
    .expect("non-zero bounds");
    analyze(&context, &kind.changed(), Some(kind.change_type()), limits)
        .unwrap_or_else(|error| panic!("the derived findings satisfy their rules: {error}"))
}

/// A change set for the impact fixtures, so that a fixture can show what was asked
/// about rather than only what was reached.
///
/// # Panics
///
/// Panics when the change is refused.
#[must_use]
pub fn change_set(kind: ImpactKind) -> amasario_impact::ChangeSet {
    let citation = Citation::new(EvidenceType::Observation, "e-observation-1")
        .expect("a citation with a kind and an identifier");
    let change = Change::new(
        kind.changed(),
        kind.change_type(),
        format!(
            "the contract at {} was observed to differ between two boundaries",
            kind.changed().id
        ),
        vec![citation],
    )
    .unwrap_or_else(|error| panic!("the change is well formed: {error}"));
    amasario_impact::ChangeSet::new(vec![change])
}

/// The evidence records the corpus's snapshots carry.
///
/// # Panics
///
/// Panics when a record does not satisfy its class, which would mean the corpus held
/// evidence the engine would refuse.
#[must_use]
pub fn evidence() -> Vec<EvidenceRecord> {
    let mut records = Vec::new();

    let mut observation = draft(
        "e-observation-1",
        "OBSERVATION",
        "the contract's instance entry was present at this boundary",
    );
    observation.observation_note =
        Some("the instance entry was returned by the endpoint".to_owned());
    observation.validate().expect("an observation record");
    records.push(observation);

    let mut transaction = draft(
        "e-transaction-1",
        "TRANSACTION",
        "the deployment transaction is recorded in the ledger",
    );
    transaction.transaction = Some(deploy_transaction());
    transaction.ledger = Some(LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"));
    transaction.successful = Some(true);
    transaction.contract_id = Some(contract_address(0xa1).as_str().to_owned());
    transaction.validate().expect("a transaction record");
    records.push(transaction);

    let mut source = draft(
        "e-source-1",
        "SOURCE",
        "the repository declares the revision the build used",
    );
    source.repository = Some(REPOSITORY_URL.to_owned());
    source.revision = Some(REVISION_COMMIT.to_owned());
    source.validate().expect("a source record");
    records.push(source);

    let mut build = draft(
        "e-build-1",
        "BUILD",
        "the build recorded its toolchain and configuration",
    );
    build.toolchain = Some("rustc 1.93.0".to_owned());
    build.configuration_digest = Some(digest_of(0x11));
    build.validate().expect("a build record");
    records.push(build);

    let mut artifact = draft(
        "e-artifact-1",
        "ARTIFACT",
        "the built module's digest was computed over its bytes",
    );
    artifact.digest = Some(module_digest("custom-section"));
    artifact.artifact_type = Some(ArtifactType::Wasm);
    artifact.validate().expect("an artifact record");
    records.push(artifact);

    let mut deployment = draft(
        "e-deployment-1",
        "DEPLOYMENT",
        "the deployment operation is recorded in the ledger",
    );
    deployment.transaction = Some(deploy_transaction());
    deployment.ledger = Some(LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"));
    deployment.contract_id = Some(contract_address(0xa1).as_str().to_owned());
    deployment.successful = Some(true);
    deployment.validate().expect("a deployment record");
    records.push(deployment);

    records
}

/// Starts an evidence record that carries the observation boundary.
///
/// The boundary is not optional in practice even though the field is: a record that
/// names a ledger without naming the chain states a number that means nothing on its
/// own, which is the refusal the engine raises and the reason this helper exists
/// rather than five call sites each remembering to set the field.
///
/// # Panics
///
/// Panics when the record does not satisfy its class, which would mean the corpus held
/// evidence the engine would refuse.
fn draft(id: &str, class: &str, claim: &str) -> EvidenceRecord {
    let mut record = EvidenceRecord::draft(id, EvidenceClass::parse(class), claim, OBSERVED_AT);
    record.boundary = Some(boundary());
    record
}

/// The attestations the corpus's snapshots cite.
///
/// # Panics
///
/// Panics when the attestation is refused.
#[must_use]
pub fn attestations() -> Vec<Attestation> {
    vec![
        Attestation::new(
            "attestation-1",
            "Amasario Test Issuer",
            alpha(),
            "the deployed module was built from the recorded revision",
            SignatureState::PresentVerified,
            Some("ed25519 over the claim digest".to_owned()),
            vec!["e-build-1".to_owned()],
        )
        .expect("the attestation is well formed"),
    ]
}

/// The two snapshots the `snapshots` suite and the CLI's `diff` command use.
///
/// The pair differs in three ways on purpose, so that the diff has one entry of each
/// kind the specification names: an added edge, a changed module digest, and a
/// changed confidence. A pair that differed only by an appended edge would let a
/// comparison that ignored digests pass.
///
/// # Panics
///
/// Panics when a capture is refused.
#[must_use]
pub fn snapshots() -> (amasario_snapshot::Snapshot, amasario_snapshot::Snapshot) {
    let before = capture_before();
    let after = capture_after();
    (before, after)
}

fn capture(subject: EntityRef, kind: SetKind, graph_kind: GraphKind) -> Capture {
    let identity = ContractIdentity::new(
        contract_address(0xa1),
        &network(),
        ContractExecutableKind::Wasm,
        Some(module_digest("custom-section")),
        LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"),
    )
    .expect("a WASM contract's identity carries its module digest");

    let mut capture = Capture::of(identity, boundary())
        .with_dependencies(set(kind))
        .with_graph(graph_document(graph_kind))
        .with_provenance(provenance_chain(ProvenanceKind::Verified))
        .with_wasm(
            ArtifactIdentity::new(
                module_digest("custom-section"),
                ArtifactType::Wasm,
                vec!["e-artifact-1".to_owned()],
            )
            .expect("the module identity cites its evidence")
            .with_size(u64::try_from(custom_section_len()).expect("a small module")),
        )
        .with_attestations(attestations())
        .with_engine_version(env!("CARGO_PKG_VERSION"))
        .truncated(false);

    for record in evidence() {
        capture
            .add_evidence(record)
            .expect("the corpus's evidence identifiers are distinct");
    }

    let _ = subject;
    capture
}

fn custom_section_len() -> usize {
    wasm_modules::custom_section_module().len()
}

fn capture_before() -> amasario_snapshot::Snapshot {
    let mut capture = capture(alpha(), SetKind::Direct, GraphKind::Direct);
    let finding = impact_analysis(ImpactKind::Direct);
    // The `before` state carries the direct findings, so the pair's diff has a change
    // to report in the impact surface as well as in the dependency surface.
    capture = capture.with_impact(finding.findings);
    capture
        .build(OBSERVED_AT)
        .expect("the before capture holds evidence")
}

fn capture_after() -> amasario_snapshot::Snapshot {
    let mut capture = capture(alpha(), SetKind::Mixed, GraphKind::Transitive);
    let finding = impact_analysis(ImpactKind::Transitive);
    capture = capture.with_impact(finding.findings);
    capture
        .build("2026-01-02T00:00:00Z")
        .expect("the after capture holds evidence")
}

/// The contract identities the `contracts` suite reads.
///
/// The three differ in what was established, which is the point: the specification's
/// contract-identity model has to be able to say "this is a WASM contract and here is
/// its module" and "this is a Stellar Asset Contract and there is no module" and "the
/// address is known and nothing else is".
///
/// # Panics
///
/// Panics when the identity is refused.
#[must_use]
pub fn contract_identities() -> Vec<(&'static str, ContractIdentity)> {
    vec![
        (
            "wasm-contract",
            ContractIdentity::new(
                contract_address(0xa1),
                &network(),
                ContractExecutableKind::Wasm,
                Some(module_digest("custom-section")),
                LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"),
            )
            .expect("a WASM identity carries its digest")
            .observed_at(LedgerSequence::new(900).expect("a real ledger")),
        ),
        (
            "stellar-asset-contract",
            ContractIdentity::new(
                contract_address(0xb2),
                &network(),
                ContractExecutableKind::StellarAsset,
                None,
                LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger"),
            )
            .expect("an asset contract has no module"),
        ),
        // The third identity is the same address observed at a later ledger with a
        // different module, which is what an upgrade looks like. It is here because the
        // specification's identity model has to distinguish "this contract" from "this
        // contract running this module now", and only a pair can show that it does.
        (
            "wasm-contract-after-upgrade",
            ContractIdentity::new(
                contract_address(0xa1),
                &network(),
                ContractExecutableKind::Wasm,
                Some(module_digest("export-without-spec")),
                LedgerSequence::new(BOUNDARY_LEDGER + 100).expect("a real ledger"),
            )
            .expect("an upgraded identity carries the new module's digest")
            .observed_at(LedgerSequence::new(BOUNDARY_LEDGER).expect("a real ledger")),
        ),
    ]
}

/// The expected reports the `reports` suite reads.
///
/// # Panics
///
/// Panics when a report fails its own validation, which would mean the corpus held a
/// report the model would not accept.
#[must_use]
pub fn report(kind: ReportKind) -> Report {
    let mut report = Report::new(alpha())
        .with_boundary(boundary())
        .generated_at(OBSERVED_AT);

    match kind {
        ReportKind::Verified => {
            report.add_observed(
                Statement::new(
                    "the contract's module digest is recorded at the boundary",
                    Some(EntityRef::wasm(&module_digest("custom-section"))),
                    vec!["e-artifact-1".to_owned()],
                )
                .expect("a statement with a citation")
                .with_confidence(
                    Confidence::new(
                        ConfidenceLevel::Verified,
                        vec!["e-artifact-1".to_owned()],
                        Vec::new(),
                    )
                    .expect("a verified confidence"),
                ),
            );
            report.add_inferred(
                InferredStatement::new(
                    "the contract requires the contract it was observed to invoke",
                    Some(bravo()),
                    vec!["e-transaction-1".to_owned()],
                    "the invocation is recorded in a successful transaction",
                )
                .expect("an inferred statement with a basis")
                .with_confidence(
                    Confidence::new(
                        ConfidenceLevel::Verified,
                        vec!["e-transaction-1".to_owned()],
                        Vec::new(),
                    )
                    .expect("a verified confidence"),
                ),
            );
            report.add_verification(VerificationEntry::new(
                EntityRef::wasm(&module_digest("custom-section")),
                VerificationStatus::Verified,
            ));
            report.add_confidence(ConfidenceEntry {
                subject: alpha(),
                confidence: Confidence::new(
                    ConfidenceLevel::Verified,
                    vec!["e-artifact-1".to_owned()],
                    Vec::new(),
                )
                .expect("a verified confidence"),
            });
            report.add_relationship_finding(
                amasario_report::RelationshipFinding::of_edge(
                    &graph_document(GraphKind::Direct).edges[0],
                )
                .expect("the edge explains itself"),
            );
        },
        ReportKind::UnknownProvenance => {
            report.add_observed(
                Statement::new(
                    "the contract exists at the boundary",
                    Some(alpha()),
                    vec!["e-observation-1".to_owned()],
                )
                .expect("a statement with a citation"),
            );
            report.add_verification(VerificationEntry::new(
                alpha(),
                VerificationStatus::Unverified,
            ));
            report.add_unknown(
                UnknownEntry::new(
                    "which source revision the deployed module was built from",
                    UnknownReason::Unsupported,
                )
                .expect("a question and a reason")
                .with_detail("no repository or revision is recorded for this contract"),
            );
        },
        ReportKind::CyclicDependencies => {
            report = report.with_graph(graph_document(GraphKind::Cyclic));
            report.add_observed(
                Statement::new(
                    "the dependency graph contains a cycle",
                    Some(bravo()),
                    vec!["e-transaction-1".to_owned()],
                )
                .expect("a statement with a citation"),
            );
            report.add_unknown(
                UnknownEntry::new(
                    "where the cycle begins, which a cyclic graph does not determine",
                    UnknownReason::NoEvidenceTypeCanAnswer,
                )
                .expect("a question and a reason")
                .with_detail(
                    "the two contracts invoke each other, so neither is upstream of the other",
                ),
            );
        },
    }

    let failures = report.failures();
    assert!(
        failures.is_empty(),
        "the {kind:?} fixture report must satisfy the specification: {failures:#?}"
    );
    report
}

/// The expected reports the corpus holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Everything the engine could establish was established.
    Verified,
    /// The contract exists and nothing about its origin is known.
    UnknownProvenance,
    /// The dependency graph is cyclic, which the report must disclose rather than hide.
    CyclicDependencies,
}

impl ReportKind {
    /// Every kind, in the order the generator writes them.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Verified,
            Self::UnknownProvenance,
            Self::CyclicDependencies,
        ]
    }

    /// The fixture file name, without the extension.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Verified => "verified-contract",
            Self::UnknownProvenance => "unknown-provenance",
            Self::CyclicDependencies => "cyclic-dependencies",
        }
    }
}

/// Renders a value the way the generator writes it: pretty, with one trailing newline.
///
/// # Panics
///
/// Panics when the value does not serialise, which cannot happen for a document built
/// from these types.
#[must_use]
pub fn rendered<T: serde::Serialize>(value: &T) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("a document serialises");
    text.push('\n');
    text
}
