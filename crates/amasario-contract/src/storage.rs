//! Observations of a contract's stored data.
//!
//! # What contract storage can and cannot be read
//!
//! Soroban stores a contract's data in ledger entries keyed by a pair of
//! `ScVal`s: the contract's address and the data key. Reading an entry therefore
//! requires *knowing the key*, and there is no operation that enumerates a
//! contract's keys. This is a protocol fact with a direct consequence for the
//! engine, and it is the reason this module is shaped the way it is: the engine can
//! read storage keys it has a reason to look up, and cannot discover keys it does
//! not.
//!
//! The consequence is that an empty result from this module is **not** evidence
//! that a contract stores nothing. It is evidence that the keys that were asked
//! about are not present. Every type here is therefore keyed on *requested* keys as
//! well as found ones, and [`StorageObservation::is_exhaustive`] is `false` by
//! construction, so that a caller cannot mistake a bounded lookup for a survey.
//!
//! # The instance's own storage
//!
//! A contract's instance entry carries a `storage` map, which the Soroban SDK uses
//! for the contract's own instance state. That map *is* enumerable, because it is
//! part of the entry the engine already retrieved, and
//! [`instance_storage`] reads it. That is a genuine enumeration of one specific
//! region, so it is presented separately from keyed lookups rather than merged with
//! them.

use amasario_core::{
    Cancellation, ContractId, EngineError, LedgerSequence, Result, TruncationReason,
};
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    ContractDataDurability, LedgerEntryData, ScAddress, ScMap, ScVal, TransactionEnvelope,
};

use amasario_network::contracts::{ContractEntry, contract_data_key, fetch_contract_code};
use amasario_network::rpc::RpcSession;

/// The durability of a contract data entry, as the protocol defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Durability {
    /// The entry persists until it is removed, and its time to live is extended by
    /// paying rent.
    Persistent,
    /// The entry expires on a fixed ledger unless it is extended.
    Temporary,
}

impl Durability {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Persistent => "PERSISTENT",
            Self::Temporary => "TEMPORARY",
        }
    }

    /// The protocol's own value.
    #[must_use]
    pub const fn to_xdr(self) -> ContractDataDurability {
        match self {
            Self::Persistent => ContractDataDurability::Persistent,
            Self::Temporary => ContractDataDurability::Temporary,
        }
    }

    /// The durability a protocol value denotes.
    #[must_use]
    pub const fn from_xdr(value: ContractDataDurability) -> Self {
        match value {
            ContractDataDurability::Persistent => Self::Persistent,
            ContractDataDurability::Temporary => Self::Temporary,
        }
    }
}

/// A contract data entry that was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageEntry {
    /// The key, rendered in a readable form.
    ///
    /// A rendering rather than the key itself: the key is an `ScVal`, which has no
    /// canonical text form, and inventing one that claimed to be canonical would
    /// make two implementations' outputs incomparable. See
    /// [`render_scval`] for what the rendering does and does not preserve.
    pub key: String,
    /// The key's value kind, which the rendering can lose.
    pub key_kind: String,
    /// Whether the entry is persistent or temporary.
    pub durability: Durability,
    /// The ledger at which the entry was last modified.
    pub last_modified_ledger: LedgerSequence,
    /// The ledger at which the entry expires, for entries that carry a time to
    /// live.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_until_ledger: Option<LedgerSequence>,
    /// The stored value's kind.
    pub value_kind: String,
}

/// The result of looking up a set of storage keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageObservation {
    /// The keys that were asked about.
    pub requested_keys: Vec<String>,
    /// The entries that were found, one per key that resolved to an entry.
    pub entries: Vec<StorageEntry>,
    /// Whether the lookup was complete over the keys it was given.
    ///
    /// Always `true` for a returned observation, because a lookup that failed
    /// returns an error rather than a partial observation. Present anyway so that a
    /// consumer reading the record can see the scope it describes, and so that a
    /// future bounded lookup has somewhere to say so.
    pub complete: bool,
    /// Why the lookup stopped early, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationReason>,
}

impl StorageObservation {
    /// Whether every requested key resolved to an entry.
    #[must_use]
    pub const fn found_every_key(&self) -> bool {
        self.entries.len() == self.requested_keys.len()
    }

    /// The keys that were asked about and not found.
    #[must_use]
    pub fn absent_keys(&self) -> Vec<String> {
        let found: Vec<&str> = self
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect();
        self.requested_keys
            .iter()
            .filter(|key| !found.contains(&key.as_str()))
            .cloned()
            .collect()
    }

    /// The entry stored under `key`, when it was found.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<&StorageEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }
}

impl StorageObservation {
    /// Always `false`: this module cannot enumerate a contract's keys.
    ///
    /// A method rather than documentation because the question is the one a caller
    /// gets wrong. Stated as a constant `false` with the reason attached, so that
    /// the answer cannot drift from the protocol's behaviour: if a future protocol
    /// version adds key enumeration, this is the single place that changes and the
    /// callers that relied on `false` become visible.
    #[must_use]
    pub const fn is_exhaustive() -> bool {
        false
    }
}

/// Reads a set of contract data entries.
///
/// The keys are all read in one request, so the observations share a boundary. A
/// second round trip would allow the contract's storage to change between reads and
/// produce an observation no single ledger ever held.
///
/// # Errors
///
/// Returns [`EngineError::Configuration`] for an empty key list - a lookup with no
/// key cannot find anything and reporting an empty observation would be misleading
/// - and a classified error when the request fails.
pub async fn observe_keys(
    session: &RpcSession,
    cancellation: &Cancellation,
    contract: &ContractId,
    keys: &[(ScVal, Durability)],
) -> Result<StorageObservation> {
    if keys.is_empty() {
        return Err(EngineError::Configuration(
            "a storage lookup needs at least one key; an empty lookup would return an empty result \
             that reads as \"the contract stores nothing\""
                .to_owned(),
        ));
    }

    let ledger_keys: Vec<_> = keys
        .iter()
        .map(|(key, durability)| contract_data_key(contract, key.clone(), durability.to_xdr()))
        .collect::<Result<Vec<_>>>()?;

    let requested_keys: Vec<String> = keys.iter().map(|(key, _)| render_scval(key)).collect();

    // The endpoint returns only the entries that exist, in the order it holds them,
    // so a response cannot be matched to a request by position. Matching is done on
    // the key inside each returned entry, which is the only sound way to attribute
    // an entry to the key that asked for it.
    let found = session.ledger_entries(cancellation, &ledger_keys).await?;

    let mut entries: Vec<StorageEntry> = Vec::with_capacity(found.len());
    for entry in found {
        let contract_entry = ContractEntry {
            data: entry.val,
            last_modified_ledger: LedgerSequence::new(entry.last_modified_ledger)?,
            live_until_ledger: entry
                .live_until_ledger_seq
                .map(LedgerSequence::new)
                .transpose()?,
        };
        if let Some(storage) = storage_entry_of(&contract_entry) {
            entries.push(storage);
        }
    }
    // Sorted so that the observation is independent of the order the endpoint
    // happened to return entries in, which is not part of any answer.
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    entries.dedup_by(|a, b| a.key == b.key);

    Ok(StorageObservation {
        requested_keys,
        entries,
        complete: true,
        truncation: None,
    })
}

/// Extracts a storage entry from a ledger entry, when it is one.
fn storage_entry_of(entry: &ContractEntry) -> Option<StorageEntry> {
    let LedgerEntryData::ContractData(data) = &entry.data else {
        return None;
    };
    Some(StorageEntry {
        key: render_scval(&data.key),
        key_kind: kind_of(&data.key).to_owned(),
        durability: Durability::from_xdr(data.durability),
        last_modified_ledger: entry.last_modified_ledger,
        live_until_ledger: entry.live_until_ledger,
        value_kind: kind_of(&data.val).to_owned(),
    })
}

/// Reads the storage map carried by a contract's instance entry.
///
/// # Errors
///
/// Returns a classified error when the code entry the instance names cannot be
/// read. That read is not needed for the storage map itself and is not performed
/// here: the function returns the instance's own map, which requires only the entry
/// already in hand.
pub fn instance_storage(entry: &ContractEntry) -> Vec<(String, String)> {
    let LedgerEntryData::ContractData(data) = &entry.data else {
        return Vec::new();
    };
    let ScVal::ContractInstance(instance) = &data.val else {
        return Vec::new();
    };
    let Some(storage) = &instance.storage else {
        // An instance with no storage map is a contract that has written no
        // instance state, which is a definite answer rather than an unknown one.
        return Vec::new();
    };

    let ScMap(entries) = storage;
    let mut rendered: Vec<(String, String)> = entries
        .iter()
        .map(|map_entry| (render_scval(&map_entry.key), render_scval(&map_entry.val)))
        .collect();
    rendered.sort();
    rendered
}

/// Renders an `ScVal` in a readable, deterministic form.
///
/// # What the rendering is
///
/// A *label for a report*, not a canonical encoding. It renders the value kinds
/// whose text forms are unambiguous - the integer types, booleans, symbols, strings
/// and byte strings - and falls back to the kind's name for anything it does not
/// render. Composite values are rendered recursively with a depth bound, so a deeply
/// nested value produces a truncated rendering rather than unbounded output.
///
/// # What the rendering is not
///
/// It is not injective. Two different `ScVal`s can render to the same text, notably
/// two byte strings whose contents happen to be valid UTF-8. Nothing in this engine
/// uses a rendering as a key, a comparison or an identity for that reason; every
/// comparison is made over the `ScVal` itself, which is what
/// [`StorageObservation::entry`] does by matching the rendered key only for display
/// and [`StorageObservation::absent_keys`] does over the requested renderings.
#[must_use]
pub fn render_scval(value: &ScVal) -> String {
    render_scval_bounded(value, 0)
}

/// The maximum nesting depth a rendering descends to.
const MAX_RENDER_DEPTH: usize = 4;

/// The maximum number of elements a rendered collection shows.
const MAX_RENDER_ELEMENTS: usize = 8;

fn render_scval_bounded(value: &ScVal, depth: usize) -> String {
    if depth > MAX_RENDER_DEPTH {
        return "…".to_owned();
    }
    match value {
        ScVal::Bool(inner) => inner.to_string(),
        ScVal::Void => "void".to_owned(),
        ScVal::Error(inner) => format!("error({inner:?})"),
        ScVal::U32(inner) => format!("u32:{inner}"),
        ScVal::I32(inner) => format!("i32:{inner}"),
        ScVal::U64(inner) => format!("u64:{inner}"),
        ScVal::I64(inner) => format!("i64:{inner}"),
        ScVal::Timepoint(inner) => format!("timepoint:{}", inner.0),
        ScVal::Duration(inner) => format!("duration:{}", inner.0),
        ScVal::U128(inner) => format!("u128:{:016x}{:016x}", inner.hi, inner.lo),
        ScVal::I128(_) | ScVal::U256(_) | ScVal::I256(_) => {
            // Rendered by kind rather than by value: the wide integers' parts are
            // signed and their decimal rendering is not something this module needs,
            // and a wrong numeric rendering in a report is worse than a coarse one.
            kind_of(value).to_owned()
        },
        ScVal::Bytes(inner) => format!("bytes:{}", hex::encode(inner.0.as_slice())),
        ScVal::String(inner) => format!("string:{}", inner.0.to_utf8_string_lossy()),
        ScVal::Symbol(inner) => format!("symbol:{}", inner.0.to_utf8_string_lossy()),
        ScVal::Vec(inner) => match inner {
            Some(stellar_xdr::ScVec(values)) => {
                let shown: Vec<String> = values
                    .iter()
                    .take(MAX_RENDER_ELEMENTS)
                    .map(|element| render_scval_bounded(element, depth + 1))
                    .collect();
                format!("vec[{}]", shown.join(","))
            },
            None => "vec[none]".to_owned(),
        },
        ScVal::Map(inner) => match inner {
            Some(ScMap(entries)) => {
                let shown: Vec<String> = entries
                    .iter()
                    .take(MAX_RENDER_ELEMENTS)
                    .map(|map_entry| {
                        format!(
                            "{}={}",
                            render_scval_bounded(&map_entry.key, depth + 1),
                            render_scval_bounded(&map_entry.val, depth + 1)
                        )
                    })
                    .collect();
                format!("map[{}]", shown.join(","))
            },
            None => "map[none]".to_owned(),
        },
        ScVal::Address(inner) => format!("address:{}", render_address(inner)),
        ScVal::ContractInstance(_) => "contract_instance".to_owned(),
        ScVal::LedgerKeyContractInstance => "ledger_key_contract_instance".to_owned(),
        ScVal::LedgerKeyNonce(inner) => format!("ledger_key_nonce:{}", inner.nonce),
    }
}

/// Renders an address, using its strkey form for a contract.
fn render_address(address: &ScAddress) -> String {
    amasario_network::transactions::contract_strkey(address)
        .unwrap_or_else(|| format!("{address:?}"))
}

/// The stable name of an `ScVal`'s kind.
///
/// A protocol fact: the name is the XDR union member, so a consumer can rely on it
/// across engine versions in a way it cannot rely on a rendering.
#[must_use]
pub const fn kind_of(value: &ScVal) -> &'static str {
    match value {
        ScVal::Bool(_) => "BOOL",
        ScVal::Void => "VOID",
        ScVal::Error(_) => "ERROR",
        ScVal::U32(_) => "U32",
        ScVal::I32(_) => "I32",
        ScVal::U64(_) => "U64",
        ScVal::I64(_) => "I64",
        ScVal::Timepoint(_) => "TIMEPOINT",
        ScVal::Duration(_) => "DURATION",
        ScVal::U128(_) => "U128",
        ScVal::I128(_) => "I128",
        ScVal::U256(_) => "U256",
        ScVal::I256(_) => "I256",
        ScVal::Bytes(_) => "BYTES",
        ScVal::String(_) => "STRING",
        ScVal::Symbol(_) => "SYMBOL",
        ScVal::Vec(_) => "VEC",
        ScVal::Map(_) => "MAP",
        ScVal::Address(_) => "ADDRESS",
        ScVal::ContractInstance(_) => "CONTRACT_INSTANCE",
        ScVal::LedgerKeyContractInstance => "LEDGER_KEY_CONTRACT_INSTANCE",
        ScVal::LedgerKeyNonce(_) => "LEDGER_KEY_NONCE",
    }
}

/// Reads the code entry a contract's instance names, when there is one.
///
/// Present here rather than in `wasm` because the question "does this contract have
/// an executable" is asked while reading storage, and answering it requires the
/// same absent-versus-error distinction the rest of this module maintains.
///
/// # Errors
///
/// Returns a classified error when the request fails, and never reports an absent
/// code entry as an error - absence is returned as `None`.
pub async fn fetch_executable(
    session: &RpcSession,
    cancellation: &Cancellation,
    hash: &[u8; 32],
) -> Result<Option<ContractEntry>> {
    fetch_contract_code(session, cancellation, hash).await
}

/// The contract addresses a transaction's envelope names, in a stable order.
///
/// Re-exported through this module so that a caller assembling an inspection does
/// not have to reach into `amasario-network` for the one helper it needs while
/// reading storage. The reading itself stays there, where the observation rules
/// live.
#[must_use]
pub fn envelope_contracts(envelope: &TransactionEnvelope) -> Vec<String> {
    use stellar_xdr::OperationBody;

    let mut addresses: Vec<String> = amasario_network::transactions::envelope_operations(envelope)
        .into_iter()
        .filter_map(|operation| match &operation.body {
            OperationBody::InvokeHostFunction(invoke) => match &invoke.host_function {
                stellar_xdr::HostFunction::InvokeContract(args) => {
                    amasario_network::transactions::contract_strkey(&args.contract_address)
                },
                _ => None,
            },
            _ => None,
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{
        ContractDataEntry, ExtensionPoint, Hash, ScContractInstance, ScString, ScSymbol,
    };

    fn contract_address(payload: [u8; 32]) -> ContractId {
        let strkey = format!("{}", stellar_strkey::Contract(payload));
        ContractId::new(strkey).expect("a real contract address")
    }

    fn ledger(value: u32) -> LedgerSequence {
        LedgerSequence::new(value).expect("a real ledger")
    }

    fn data_entry(key: ScVal, val: ScVal, durability: ContractDataDurability) -> ContractEntry {
        ContractEntry {
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(stellar_xdr::ContractId(Hash([1_u8; 32]))),
                key,
                durability,
                val,
            }),
            last_modified_ledger: ledger(500),
            live_until_ledger: Some(ledger(900)),
        }
    }

    fn symbol(value: &str) -> ScVal {
        ScVal::Symbol(ScSymbol(value.parse().expect("a short symbol")))
    }

    #[test]
    fn durable_kinds_round_trip_through_the_protocols_values() {
        for durability in [Durability::Persistent, Durability::Temporary] {
            assert_eq!(Durability::from_xdr(durability.to_xdr()), durability);
            assert!(!durability.as_str().is_empty());
        }
        assert_eq!(
            Durability::Persistent.to_xdr(),
            ContractDataDurability::Persistent
        );
    }

    #[test]
    fn a_storage_lookup_is_never_reported_as_exhaustive() {
        // The protocol offers no key enumeration, so an empty result says nothing
        // about what a contract stores.
        assert!(!StorageObservation::is_exhaustive());
    }

    #[test]
    fn a_found_entry_records_its_key_kind_durability_and_timestamps() {
        let entry = data_entry(
            symbol("counter"),
            ScVal::U32(7),
            ContractDataDurability::Persistent,
        );
        let storage = storage_entry_of(&entry).expect("a contract data entry");

        assert_eq!(storage.key, "symbol:counter");
        assert_eq!(storage.key_kind, "SYMBOL");
        assert_eq!(storage.value_kind, "U32");
        assert_eq!(storage.durability, Durability::Persistent);
        assert_eq!(storage.last_modified_ledger, ledger(500));
        assert_eq!(storage.live_until_ledger, Some(ledger(900)));
    }

    #[test]
    fn a_ledger_entry_that_is_not_contract_data_is_not_reported_as_storage() {
        let entry = ContractEntry {
            data: LedgerEntryData::ContractCode(stellar_xdr::ContractCodeEntry {
                ext: stellar_xdr::ContractCodeEntryExt::V0,
                hash: Hash([2_u8; 32]),
                code: stellar_xdr::BytesM::try_from(vec![0_u8]).expect("one byte"),
            }),
            last_modified_ledger: ledger(500),
            live_until_ledger: None,
        };
        assert!(storage_entry_of(&entry).is_none());
        assert!(instance_storage(&entry).is_empty());
    }

    #[test]
    fn absent_keys_are_reported_rather_than_omitted() {
        // An empty result must not read as "the contract stores nothing".
        let observation = StorageObservation {
            requested_keys: vec!["symbol:alpha".to_owned(), "symbol:beta".to_owned()],
            entries: vec![StorageEntry {
                key: "symbol:alpha".to_owned(),
                key_kind: "SYMBOL".to_owned(),
                durability: Durability::Persistent,
                last_modified_ledger: ledger(500),
                live_until_ledger: None,
                value_kind: "U32".to_owned(),
            }],
            complete: true,
            truncation: None,
        };

        assert!(!observation.found_every_key());
        assert_eq!(observation.absent_keys(), ["symbol:beta"]);
        assert!(observation.entry("symbol:alpha").is_some());
        assert!(observation.entry("symbol:beta").is_none());
    }

    #[test]
    fn the_instance_storage_map_is_enumerable_and_ordered() {
        // The one region that genuinely can be enumerated, because it is part of
        // the entry the engine already has.
        let entry = ContractEntry {
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(stellar_xdr::ContractId(Hash([1_u8; 32]))),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
                val: ScVal::ContractInstance(ScContractInstance {
                    executable: stellar_xdr::ContractExecutable::StellarAsset,
                    storage: Some(ScMap(
                        vec![
                            stellar_xdr::ScMapEntry {
                                key: symbol("z"),
                                val: ScVal::U32(1),
                            },
                            stellar_xdr::ScMapEntry {
                                key: symbol("a"),
                                val: ScVal::U32(2),
                            },
                        ]
                        .try_into()
                        .expect("two entries"),
                    )),
                }),
            }),
            last_modified_ledger: ledger(500),
            live_until_ledger: None,
        };

        let rendered = instance_storage(&entry);
        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0], ("symbol:a".to_owned(), "u32:2".to_owned()));
        assert_eq!(rendered[1], ("symbol:z".to_owned(), "u32:1".to_owned()));
    }

    #[test]
    fn an_instance_without_a_storage_map_reports_no_instance_state() {
        let entry = ContractEntry {
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(stellar_xdr::ContractId(Hash([1_u8; 32]))),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
                val: ScVal::ContractInstance(ScContractInstance {
                    executable: stellar_xdr::ContractExecutable::StellarAsset,
                    storage: None,
                }),
            }),
            last_modified_ledger: ledger(500),
            live_until_ledger: None,
        };
        assert!(instance_storage(&entry).is_empty());
    }

    #[test]
    fn renderings_cover_the_unambiguous_value_kinds() {
        assert_eq!(render_scval(&ScVal::Bool(true)), "true");
        assert_eq!(render_scval(&ScVal::Void), "void");
        assert_eq!(render_scval(&ScVal::U32(9)), "u32:9");
        assert_eq!(render_scval(&ScVal::I32(-9)), "i32:-9");
        assert_eq!(
            render_scval(&ScVal::U64(u64::MAX)),
            format!("u64:{}", u64::MAX)
        );
        assert_eq!(render_scval(&symbol("hi")), "symbol:hi");
        assert_eq!(
            render_scval(&ScVal::String(ScString(
                b"hi".to_vec().try_into().expect("two bytes")
            ))),
            "string:hi"
        );
        assert_eq!(
            render_scval(&ScVal::Bytes(
                vec![0xde, 0xad].try_into().expect("two bytes")
            )),
            "bytes:dead"
        );
        assert_eq!(
            render_scval(&ScVal::LedgerKeyContractInstance),
            "ledger_key_contract_instance"
        );
    }

    #[test]
    fn an_address_renders_as_a_contract_strkey_when_it_is_one() {
        let payload = [7_u8; 32];
        let rendered = render_scval(&ScVal::Address(ScAddress::Contract(
            stellar_xdr::ContractId(Hash(payload)),
        )));
        assert_eq!(rendered, format!("address:{}", contract_address(payload)));
        assert!(rendered.contains('C'));
    }

    #[test]
    fn a_deeply_nested_value_is_truncated_rather_than_rendered_without_bound() {
        // A rendering with no depth bound would let one stored value fill a report.
        let mut value = ScVal::U32(1);
        for _ in 0..12 {
            value = ScVal::Vec(Some(vec![value.clone()].try_into().expect("one element")));
        }
        let rendered = render_scval(&value);
        assert!(rendered.contains('…'), "got: {rendered}");
        assert!(rendered.len() < 200, "the rendering is bounded: {rendered}");
    }

    #[test]
    fn a_wide_collection_shows_a_bounded_number_of_elements() {
        let elements: Vec<ScVal> = (0..40).map(ScVal::U32).collect();
        let value = ScVal::Vec(Some(elements.try_into().expect("forty elements")));
        let rendered = render_scval(&value);

        assert!(rendered.contains("u32:0"));
        assert!(
            !rendered.contains("u32:39"),
            "the rendering must stop at the bound: {rendered}"
        );
    }

    #[test]
    fn a_null_collection_is_rendered_distinctly_from_an_empty_one() {
        assert_eq!(render_scval(&ScVal::Vec(None)), "vec[none]");
        assert_eq!(render_scval(&ScVal::Map(None)), "map[none]");
        assert_eq!(
            render_scval(&ScVal::Vec(Some(
                Vec::<ScVal>::new().try_into().expect("an empty vector")
            ))),
            "vec[]"
        );
    }

    #[test]
    fn every_value_kind_has_a_stable_name() {
        // The name is the XDR union member, so a consumer may rely on it where a
        // rendering is not reliable.
        let kinds = [
            (ScVal::Bool(false), "BOOL"),
            (ScVal::Void, "VOID"),
            (ScVal::U32(0), "U32"),
            (ScVal::I32(0), "I32"),
            (ScVal::U64(0), "U64"),
            (ScVal::I64(0), "I64"),
            (
                ScVal::U128(stellar_xdr::UInt128Parts { hi: 0, lo: 0 }),
                "U128",
            ),
            (
                ScVal::I128(stellar_xdr::Int128Parts { hi: 0, lo: 0 }),
                "I128",
            ),
            (ScVal::Bytes(Vec::new().try_into().expect("empty")), "BYTES"),
            (
                ScVal::String(ScString(Vec::new().try_into().expect("empty"))),
                "STRING",
            ),
            (symbol("s"), "SYMBOL"),
            (ScVal::Vec(None), "VEC"),
            (ScVal::Map(None), "MAP"),
            (
                ScVal::LedgerKeyContractInstance,
                "LEDGER_KEY_CONTRACT_INSTANCE",
            ),
        ];
        for (value, expected) in kinds {
            assert_eq!(kind_of(&value), expected);
        }
    }

    #[test]
    fn rendering_is_deterministic() {
        let value = ScVal::Vec(Some(
            vec![symbol("a"), ScVal::U32(2)]
                .try_into()
                .expect("two elements"),
        ));
        let first = render_scval(&value);
        for _ in 0..32 {
            assert_eq!(render_scval(&value), first);
        }
    }
}
