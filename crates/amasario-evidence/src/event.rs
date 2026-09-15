//! Event evidence: what a contract was observed to do during one execution.
//!
//! # Why events are the runtime observable
//!
//! `taxonomies/evidence-types.yaml` calls events "the primary observable for cross-contract
//! invocation, so this type is central to runtime dependency discovery". An event is the
//! only record of a cross-contract call that the chain itself emits, and it is why
//! [`EventOrigin`] requires both the emitting transaction and the event's index within it:
//! without the transaction the event cannot be found again, and without the index it cannot
//! be told apart from its siblings. That pair is what the taxonomy's `traceableTo` names,
//! so the type cannot be constructed without it.
//!
//! # An event and a claim are different statements
//!
//! [`from_event`] takes the claim rather than deriving it. An event whose topics name a
//! callee supports a claim about a runtime dependency, and the same event supports nothing
//! at all about the callee's source: the meaning a claim takes from an event is the
//! collector's statement, not the engine's. The observation itself is carried in
//! [`EventOrigin::note`] and copied into the record's separate `observation_note`, so a
//! report can show what was seen next to what it was cited for and a reader can disagree
//! with the second without disputing the first.

use amasario_core::{ContractId, LedgerSequence, ObservationBoundary, Result, TransactionHash};

use crate::collector::{EvidenceClass, EvidenceRecord};

/// One contract event, with the origin that makes it locatable.
///
/// The index is a `u32` rather than an `Option`, matching `schema/event.schema.json`'s
/// `eventIndex` minimum of zero: an event that cannot be positioned within its transaction
/// is not evidence of which event occurred, so there is no useful record to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventOrigin {
    /// The contract that emitted the event.
    pub contract: ContractId,
    /// The transaction the event was emitted during.
    pub transaction: TransactionHash,
    /// The event's index within its emitting transaction.
    pub event_index: u32,
    /// The boundary the event was read at.
    pub boundary: ObservationBoundary,
    /// What the event was observed to carry, stated factually.
    pub note: String,
}

impl EventOrigin {
    /// Records an event's origin.
    #[must_use]
    pub fn new(
        contract: ContractId,
        transaction: TransactionHash,
        event_index: u32,
        boundary: ObservationBoundary,
        note: impl Into<String>,
    ) -> Self {
        Self {
            contract,
            transaction,
            event_index,
            boundary,
            note: note.into(),
        }
    }

    /// The ledger the event was emitted in.
    #[must_use]
    pub const fn ledger(&self) -> LedgerSequence {
        self.boundary.ledger
    }
}

/// A record for an event, cited for a claim.
///
/// # Errors
///
/// Returns a provenance error when the record fails its class, which includes an event
/// whose note is too short to say what was observed. An observation that says nothing is
/// not an observation, and recording one would let a claim cite a note that carries no
/// information.
pub fn from_event(
    origin: &EventOrigin,
    claim: impl Into<String>,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record = EvidenceRecord::draft(id, EvidenceClass::parse("EVENT"), claim, observed_at);
    record.contract_id = Some(origin.contract.to_string());
    record.transaction = Some(origin.transaction.clone());
    record.ledger = Some(origin.ledger());
    record.event_index = Some(origin.event_index);
    record.boundary = Some(origin.boundary.clone());
    // The note is the observation, and it is kept separate from the claim so that a report
    // can distinguish what was seen from what it was cited for.
    record.observation_note = Some(origin.note.clone());
    record.validate()?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::{EvidenceBasis, basis_for};
    use amasario_core::{Network, NetworkType};

    const PASSPHRASE: &str = "Test SDF Network ; September 2015";

    fn boundary() -> ObservationBoundary {
        ObservationBoundary::new(
            Network::new("testnet", NetworkType::Testnet, PASSPHRASE).expect("a network"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn contract() -> ContractId {
        ContractId::new("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM")
            .expect("a contract address")
    }

    fn origin() -> EventOrigin {
        EventOrigin::new(
            contract(),
            TransactionHash::new("cd".repeat(32)).expect("a transaction hash"),
            3,
            boundary(),
            "the event's topics name a callee contract".to_owned(),
        )
    }

    #[test]
    fn an_event_record_names_its_origin_and_its_observation() {
        let record = from_event(
            &origin(),
            "the emitting contract invoked the callee named in the event's topics",
            "e-event",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable event record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(record.event_index, Some(3));
        assert_eq!(record.ledger.expect("a ledger").get(), 1_000);
        assert_eq!(
            record.contract_id.as_deref(),
            Some("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM")
        );
        assert_eq!(
            record.observation_note.as_deref(),
            Some("the event's topics name a callee contract"),
            "what was seen is kept separate from what it was cited for"
        );
        assert_ne!(
            record.observation_note.as_deref(),
            Some(record.claim.as_str())
        );
        assert_eq!(basis_for(&record), EvidenceBasis::Authoritative);
        assert_eq!(origin().ledger().get(), 1_000);
    }

    #[test]
    fn an_event_note_too_short_to_say_anything_is_refused() {
        let short = EventOrigin::new(
            contract(),
            TransactionHash::new("cd".repeat(32)).expect("a transaction hash"),
            0,
            boundary(),
            "seen",
        );
        let error = from_event(
            &short,
            "the emitting contract invoked another contract",
            "e-event",
            "2026-01-01T00:00:00Z",
        )
        .expect_err("an observation that says nothing is not an observation");
        assert!(
            error
                .to_string()
                .contains("does not state what was observed")
        );
    }

    #[test]
    fn two_events_in_one_transaction_are_distinguishable() {
        // The index is the second half of the identity the taxonomy names. Without it the
        // two records would describe the same location and neither could be re-checked.
        let first = EventOrigin::new(
            contract(),
            TransactionHash::new("cd".repeat(32)).expect("a transaction hash"),
            0,
            boundary(),
            "the event's topics name a callee contract".to_owned(),
        );
        let second = EventOrigin::new(
            contract(),
            TransactionHash::new("cd".repeat(32)).expect("a transaction hash"),
            1,
            boundary(),
            "the event's topics name a different callee".to_owned(),
        );
        let a = from_event(
            &first,
            "one contract invoked a callee",
            "e-0",
            "2026-01-01T00:00:00Z",
        )
        .expect("a record");
        let b = from_event(
            &second,
            "one contract invoked a callee",
            "e-1",
            "2026-01-01T00:00:00Z",
        )
        .expect("a record");
        assert_ne!(
            (a.transaction.as_ref(), a.event_index),
            (b.transaction.as_ref(), b.event_index),
            "the emitting transaction and the index together locate the event"
        );
        assert_eq!(a.transaction, b.transaction);
    }
}
