//! The contract-level view of a dependency set.
//!
//! # Why a projection rather than a second model
//!
//! `dependency/contract-dependency` requires a `CONTRACT` dependency to name the
//! network its target was observed on, and the reason is that a contract address
//! identifies a contract only together with its network. The set already records the
//! boundary each dependency was established at, so this module does not re-derive
//! anything: it presents the contract-level dependencies with the network pulled out
//! of the boundary, the transactions that showed them pulled out of the evidence, and
//! the addresses checked for the one property a reader needs in order to look the
//! target up at all - that the identifier is a well-formed contract address.
//!
//! # Why an unparseable address is reported rather than rejected
//!
//! A dependency on a target whose identifier is not a contract address is still a
//! record of something that was observed. The honest output is the dependency plus the
//! note that the address cannot be resolved, because a consumer that cannot look up
//! the target must be told so. Silently dropping it would make the analysis look
//! tidier than the evidence allows.

use amasario_core::{ContractId, DependencyClass, Network, VerificationStatus};

use crate::resolver::{Dependency, DependencySet};

/// A dependency whose object is a Soroban contract, in the shape a reader needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractDependency {
    /// The calling contract.
    pub subject: String,
    /// The contract depended upon.
    pub contract: String,
    /// The network the dependency was observed on.
    pub network: String,
    /// The classes the evidence supports.
    pub classes: Vec<DependencyClass>,
    /// What the evidence says about the dependency.
    pub verification: VerificationStatus,
    /// The confidence level the basis supports.
    pub confidence: amasario_core::ConfidenceLevel,
    /// The transactions that showed the dependency, in citation order.
    pub transactions: Vec<String>,
    /// Why the dependency exists, in terms of the evidence.
    pub reason: String,
    /// Whether the target's identifier is a well-formed contract address.
    ///
    /// False means the dependency is recorded but its target cannot be looked up, and
    /// a consumer has to be told rather than left to discover it.
    pub address_is_resolvable: bool,
}

impl ContractDependency {
    /// Whether the target can be looked up on its network.
    #[must_use]
    pub const fn is_actionable(&self) -> bool {
        self.address_is_resolvable
    }
}

/// The contract-level dependencies of a set, in the set's own order.
///
/// Includes the direct and transitive partitions, because a contract that is two hops
/// away is still a contract in the blast radius, and its network is still what a
/// reader needs in order to check it.
#[must_use]
pub fn contract_dependencies(set: &DependencySet) -> Vec<ContractDependency> {
    set.all()
        .filter(|dependency| dependency.classes.contains(&DependencyClass::Contract))
        .map(|dependency| ContractDependency {
            subject: dependency.subject.id.clone(),
            contract: dependency.object.id.clone(),
            // The classifier refuses a CONTRACT dependency without a network, so this
            // fallback is unreachable through the engine's own paths. It exists so that
            // a hand-assembled set produces a visible placeholder rather than an empty
            // string that reads like a real network name.
            network: dependency
                .network()
                .map_or_else(|| "<unrecorded>".to_owned(), |network| network.id.clone()),
            classes: dependency.classes.clone(),
            verification: dependency.verification,
            confidence: dependency.confidence.level,
            transactions: dependency
                .transactions()
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            reason: dependency.reason.clone(),
            address_is_resolvable: ContractId::new(&dependency.object.id).is_ok(),
        })
        .collect()
}

/// The dependencies a contract depends on only while executing.
///
/// Separated from the rest because it answers a different operational question: a
/// runtime dependency is one whose target has to be reachable for the subject to work,
/// and it rests on an observed transaction rather than on a declaration.
#[must_use]
pub fn runtime_dependencies(set: &DependencySet) -> Vec<&Dependency> {
    set.with_class(DependencyClass::Runtime)
}

/// The contract-level dependencies whose target cannot be looked up.
///
/// Not an error list: each entry is a real finding whose target is not addressable,
/// which is what a reader needs to know before attempting to resolve it.
#[must_use]
pub fn unresolvable(set: &DependencySet) -> Vec<ContractDependency> {
    contract_dependencies(set)
        .into_iter()
        .filter(|dependency| !dependency.address_is_resolvable)
        .collect()
}

/// The networks a set's dependencies were observed on, deduplicated and ordered.
#[must_use]
pub fn networks(set: &DependencySet) -> Vec<&Network> {
    let mut networks: Vec<&Network> = Vec::new();
    for dependency in set.all() {
        if let Some(network) = dependency.network()
            && !networks.iter().any(|known| known.id == network.id)
        {
            networks.push(network);
        }
    }
    networks.sort_by(|left, right| left.id.cmp(&right.id));
    networks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::{Candidate, EvidenceRef};
    use amasario_core::{
        Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, NetworkType,
        ObservationBoundary, Relationship,
    };

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(6_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn subject() -> EntityRef {
        EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference")
    }

    fn invocation(callee: &str, transaction: String) -> Candidate {
        Candidate::new(
            subject(),
            EntityRef::new(EntityKind::Contract, callee).expect("a reference"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    fn resolve(candidates: &[Candidate]) -> DependencySet {
        crate::resolver::resolve(subject(), Some(boundary()), candidates, 5).expect("resolves")
    }

    #[test]
    fn a_contract_dependency_carries_its_network_and_its_transactions() {
        let set = resolve(&[invocation("C-callee", "a".repeat(64))]);
        let dependencies = contract_dependencies(&set);
        assert_eq!(dependencies.len(), 1);
        let dependency = &dependencies[0];
        assert_eq!(dependency.subject, "C-subject");
        assert_eq!(dependency.contract, "C-callee");
        assert_eq!(dependency.network, "testnet");
        assert_eq!(dependency.transactions, vec!["a".repeat(64)]);
        assert!(dependency.classes.contains(&DependencyClass::Runtime));
        assert_eq!(dependency.verification, VerificationStatus::Verified);
        assert!(dependency.reason.contains("INVOCATES"));
    }

    #[test]
    fn a_target_that_is_not_a_contract_address_is_reported_as_unresolvable() {
        // Recorded, not dropped: a consumer that cannot look the target up has to be
        // told, and the dependency is still a record of something observed.
        let set = resolve(&[invocation("not-an-address", "b".repeat(64))]);
        let dependencies = contract_dependencies(&set);
        assert_eq!(dependencies.len(), 1);
        assert!(!dependencies[0].address_is_resolvable);
        assert!(!dependencies[0].is_actionable());
        assert_eq!(unresolvable(&set).len(), 1);
        assert_eq!(set.len(), 1, "the dependency is still in the set");
    }

    #[test]
    fn a_well_formed_contract_address_is_actionable() {
        // A real strkey contract address, so that the check is exercised against the
        // encoding rather than against a string that merely looks plausible.
        let address = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
        let set = resolve(&[invocation(address, "c".repeat(64))]);
        let dependencies = contract_dependencies(&set);
        assert!(dependencies[0].address_is_resolvable);
        assert!(unresolvable(&set).is_empty());
    }

    #[test]
    fn runtime_dependencies_are_separated_from_the_rest() {
        let set = resolve(&[
            invocation("C-callee", "d".repeat(64)),
            Candidate::new(
                subject(),
                EntityRef::new(EntityKind::Package, "soroban-sdk").expect("a reference"),
                Relationship::DependsOn,
                Basis::ResolvedLockfile,
                vec![EvidenceRef::new(EvidenceType::Build, "lock-1").expect("a citation")],
            )
            .expect("a candidate")
            .observed_at(boundary()),
        ]);
        let runtime = runtime_dependencies(&set);
        assert_eq!(runtime.len(), 1);
        assert_eq!(runtime[0].object.id, "C-callee");
        // A package dependency is not a contract dependency, so the contract view holds
        // one entry rather than two.
        assert_eq!(contract_dependencies(&set).len(), 1);
    }

    #[test]
    fn the_networks_behind_a_set_are_deduplicated_and_ordered() {
        let set = resolve(&[
            invocation("C-a", "e".repeat(64)),
            invocation("C-b", "f".repeat(64)),
        ]);
        let networks = networks(&set);
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].id, "testnet");
    }

    #[test]
    fn an_empty_set_projects_to_nothing_without_inventing_a_network() {
        let set = resolve(&[]);
        assert!(contract_dependencies(&set).is_empty());
        assert!(runtime_dependencies(&set).is_empty());
        assert!(unresolvable(&set).is_empty());
        assert!(networks(&set).is_empty());
    }
}
