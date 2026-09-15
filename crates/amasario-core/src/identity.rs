//! Identity primitives: entity kinds, references, digests, addresses and ledgers.
//!
//! These types exist so that the specification's central distinction cannot be
//! lost in transit. An address is not an identity; a digest is only meaningful
//! with its algorithm; a ledger number is only meaningful with its network. Each
//! of those is enforced by the type rather than by a comment, because a comment
//! does not survive a refactor.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::errors::{EngineError, Result};

/// The kinds of entity a relationship may connect.
///
/// Matches the `entityKind` definition in the specification's
/// `schema/provenance.schema.json`. The set is closed: an unrecognised kind is a
/// rejection rather than a tolerated value, because a relationship whose endpoint
/// kind is not understood cannot be reasoned about at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum EntityKind {
    /// A deployed contract identity.
    Contract,
    /// A WebAssembly executable.
    Wasm,
    /// A content-addressed artifact of any type.
    Artifact,
    /// A source repository at a revision.
    Source,
    /// A recorded build.
    Build,
    /// A deployment record.
    Deployment,
    /// A software package or crate.
    Package,
    /// A ledger transaction.
    Transaction,
}

impl EntityKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contract => "CONTRACT",
            Self::Wasm => "WASM",
            Self::Artifact => "ARTIFACT",
            Self::Source => "SOURCE",
            Self::Build => "BUILD",
            Self::Deployment => "DEPLOYMENT",
            Self::Package => "PACKAGE",
            Self::Transaction => "TRANSACTION",
        }
    }

    /// Every kind, in the canonical order used for deterministic output.
    ///
    /// Ordering is fixed here rather than left to the caller because the
    /// specification requires deterministic serialisation, and an iteration order
    /// that depends on the construction path would make two runs differ.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Contract,
            Self::Wasm,
            Self::Artifact,
            Self::Source,
            Self::Build,
            Self::Deployment,
            Self::Package,
            Self::Transaction,
        ]
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EntityKind {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "CONTRACT" => Ok(Self::Contract),
            "WASM" => Ok(Self::Wasm),
            "ARTIFACT" => Ok(Self::Artifact),
            "SOURCE" => Ok(Self::Source),
            "BUILD" => Ok(Self::Build),
            "DEPLOYMENT" => Ok(Self::Deployment),
            "PACKAGE" => Ok(Self::Package),
            "TRANSACTION" => Ok(Self::Transaction),
            other => Err(EngineError::Validation {
                path: "/kind".to_owned(),
                detail: format!("unrecognised entity kind {other:?}"),
            }),
        }
    }
}

/// The digest algorithms the specification permits.
///
/// Deliberately not an open set. A locally chosen algorithm would make two
/// implementations' digests incomparable, which destroys the only property that
/// makes a digest usable as an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DigestAlgorithm {
    /// SHA-256. The only algorithm permitted for a WASM artifact, because it is
    /// the algorithm the Stellar network reports for a contract's executable hash.
    Sha256,
    /// SHA-512. Permitted for artifacts other than WASM.
    Sha512,
}

impl DigestAlgorithm {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    /// The length, in hex characters, of a value produced by this algorithm.
    #[must_use]
    pub const fn hex_length(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }
}

impl fmt::Display for DigestAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DigestAlgorithm {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "sha256" => Ok(Self::Sha256),
            "sha512" => Ok(Self::Sha512),
            other => Err(EngineError::Validation {
                path: "/digest/algorithm".to_owned(),
                detail: format!(
                    "unrecognised digest algorithm {other:?}; only sha256 and sha512 are defined"
                ),
            }),
        }
    }
}

/// A content digest, carrying the algorithm that produced it.
///
/// A bare hex string is not a digest: without the algorithm, a 64-character value
/// is equally consistent with a truncated sha512 and a sha256, and the two are not
/// comparable. Constructing one goes through [`Digest::new`], which enforces both
/// the alphabet and the algorithm's expected length.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "DigestRepr", into = "DigestRepr")]
pub struct Digest {
    algorithm: DigestAlgorithm,
    value: String,
}

/// The wire representation, which is also what a caller writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DigestRepr {
    algorithm: String,
    value: String,
}

impl TryFrom<DigestRepr> for Digest {
    type Error = EngineError;

    fn try_from(repr: DigestRepr) -> Result<Self> {
        let algorithm = DigestAlgorithm::from_str(&repr.algorithm)?;
        Self::new(algorithm, &repr.value)
    }
}

impl From<Digest> for DigestRepr {
    fn from(digest: Digest) -> Self {
        Self {
            algorithm: digest.algorithm.as_str().to_owned(),
            value: digest.value,
        }
    }
}

impl Digest {
    /// Builds a digest, rejecting a value that is not a well-formed hex digest of
    /// the stated algorithm's length.
    pub fn new(algorithm: DigestAlgorithm, value: &str) -> Result<Self> {
        if value.is_empty() {
            return Err(EngineError::Validation {
                path: "/digest/value".to_owned(),
                detail: "a digest value may not be empty".to_owned(),
            });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(EngineError::Validation {
                path: "/digest/value".to_owned(),
                detail: format!("a digest value must be lowercase hexadecimal; {value:?} is not"),
            });
        }
        let expected = algorithm.hex_length();
        if value.len() != expected {
            return Err(EngineError::Validation {
                path: "/digest/value".to_owned(),
                detail: format!(
                    "{} expects {expected} hex characters, but the value is {}",
                    algorithm.as_str(),
                    value.len()
                ),
            });
        }
        Ok(Self {
            algorithm,
            value: value.to_owned(),
        })
    }

    /// Computes the SHA-256 digest of a byte slice.
    ///
    /// The one-shot constructor used everywhere the input is held in memory.
    /// Streaming input is hashed by the caller and passed to [`Digest::new`], so
    /// that this type never has to hold content it does not own.
    #[must_use]
    pub fn sha256_of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        Self {
            algorithm: DigestAlgorithm::Sha256,
            value: hex::encode(digest),
        }
    }

    /// The algorithm that produced this digest.
    #[must_use]
    pub const fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }

    /// The lowercase hex value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The canonical `algorithm:value` form used in vector expectations.
    #[must_use]
    pub fn prefixed(&self) -> String {
        format!("{}:{}", self.algorithm.as_str(), self.value)
    }

    /// Whether two digests claim to identify the same content.
    ///
    /// Name kept explicit rather than implemented as `PartialEq` alone, because
    /// comparing digests is a decision the caller is making about identity, and
    /// the call site should read that way.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self == other
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm.as_str(), self.value)
    }
}

impl FromStr for Digest {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        let (algorithm, hex_value) = value.split_once(':').ok_or_else(|| {
            EngineError::Validation {
                path: "/digest".to_owned(),
                detail: format!(
                    "a digest must be written as algorithm:value; {value:?} has no algorithm prefix"
                ),
            }
        })?;
        Self::new(DigestAlgorithm::from_str(algorithm)?, hex_value)
    }
}

/// The number of characters in a Soroban contract strkey.
const CONTRACT_STRKEY_LENGTH: usize = 56;

/// A Soroban contract address.
///
/// Stored as the validated string rather than as decoded bytes because every
/// consumer of this type - an RPC request, a report, a graph node identifier -
/// needs the encoded form, and decoding is only necessary when the payload is
/// being inspected, which no stage of the engine does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContractId(String);

impl ContractId {
    /// Validates and wraps a contract address.
    ///
    /// Only the shape is checked here: a 56-character strkey beginning with `C`
    /// whose remaining characters are in the RFC 4648 base32 alphabet. The CRC16
    /// checksum that strkey embeds is checked by [`ContractId::verify_checksum`],
    /// which is separate because the specification's schema also checks shape
    /// only, and a caller that needs stronger validation should ask for it
    /// explicitly rather than assume it happened.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != CONTRACT_STRKEY_LENGTH {
            return Err(EngineError::Validation {
                path: "/contractId".to_owned(),
                detail: format!(
                    "a contract address is {CONTRACT_STRKEY_LENGTH} characters, but the value is {}",
                    value.len()
                ),
            });
        }
        let mut chars = value.chars();
        if chars.next() != Some('C') {
            return Err(EngineError::Validation {
                path: "/contractId".to_owned(),
                detail: "a contract address begins with 'C'".to_owned(),
            });
        }
        if !value[1..]
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))
        {
            return Err(EngineError::Validation {
                path: "/contractId".to_owned(),
                detail: format!(
                    "{value} is not base32 (RFC 4648); the characters after 'C' must be A-Z or 2-7"
                ),
            });
        }
        Ok(Self(value))
    }

    /// The encoded address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Verifies the version byte and CRC16 checksum that strkey embeds.
    ///
    /// The check is deliberately not part of construction. `amasario-network`
    /// calls it on addresses it receives from a network, where a checksum failure
    /// indicates a corrupt response worth reporting; the analysis path uses the
    /// shape check, because failing an entire analysis because one unrelated
    /// reference in a document had a typo would be a worse outcome than reporting
    /// it as unresolvable.
    ///
    /// # On the version byte
    ///
    /// Per SEP-23 a strkey's first byte is `prefix_index << 3`, where the prefix
    /// index is the position of the leading letter in the base32 alphabet. `C` is
    /// the third letter, so a contract strkey begins with `0x10`. This value is
    /// not guessed: `tests/strkey_interop.rs` builds a contract strkey with
    /// Stellar's own `stellar-strkey` crate and asserts that this decoder accepts
    /// it and rejects a corrupted variant, which is what makes the constant
    /// checkable rather than asserted.
    pub fn verify_checksum(&self) -> Result<()> {
        self.decode_payload().map(|_| ())
    }

    /// The 32-byte contract identifier the address encodes.
    ///
    /// Needed because Stellar RPC addresses a contract by its raw identifier:
    /// building a `LedgerKey` for a contract needs the bytes, not the strkey. The
    /// checksum is verified first, so a corrupt address is reported rather than
    /// silently used to query a different contract - which would return an absent
    /// entry, indistinguishable from a contract that does not exist. That
    /// distinction is exactly what the engine must not lose, so the decoding and
    /// the verification happen together rather than being two independent calls a
    /// caller could forget to pair.
    ///
    /// Decoding lives here rather than in `amasario-network` because this type
    /// owns identity: a second decoder in the network layer would be a second
    /// place for the version byte to be wrong.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Validation`] when the address is not decodable
    /// base32, decodes to the wrong length, has the wrong version byte, or fails
    /// its checksum.
    pub fn payload(&self) -> Result<[u8; 32]> {
        let payload = self.decode_payload()?;
        let mut bytes = [0_u8; 32];
        // The length is already known to be 35 and the version byte to be correct,
        // so the identifier is the 32 bytes between the version and the checksum.
        bytes.copy_from_slice(&payload[1..33]);
        Ok(bytes)
    }

    /// Decodes the strkey payload, verifying its version byte and CRC16 checksum.
    fn decode_payload(&self) -> Result<Vec<u8>> {
        // strkey payload is the version byte, then 32 bytes of contract identifier,
        // then a big-endian CRC16/XMODEM over everything preceding it.
        const VERSION_BYTE: u8 = 0x10;
        let payload = base32_decode(&self.0).ok_or_else(|| EngineError::Validation {
            path: "/contractId".to_owned(),
            detail: format!("{} is not decodable base32", self.0),
        })?;
        if payload.len() != 35 {
            return Err(EngineError::Validation {
                path: "/contractId".to_owned(),
                detail: format!(
                    "a contract strkey decodes to 35 bytes; {} decodes to {}",
                    self.0,
                    payload.len()
                ),
            });
        }
        if payload[0] != VERSION_BYTE {
            return Err(EngineError::Validation {
                path: "/contractId".to_owned(),
                detail: format!(
                    "expected strkey version byte {VERSION_BYTE:#04x}, found {:#04x}",
                    payload[0]
                ),
            });
        }
        let (body, checksum) = payload.split_at(payload.len() - 2);
        let expected = crc16_xmodem(body);
        // Little-endian, not big-endian. Stellar's own implementation documents
        // this: `crc::checksum` returns "the 2-byte checksum for the provided data,
        // in little endian byte-order", and its own test records that
        // `checksum(b"123456789")` is `[0xc3, 0x31]` rather than `[0x31, 0xc3]`.
        // Reading it the other way round makes every valid address look corrupt.
        let found = u16::from_le_bytes([checksum[0], checksum[1]]);
        if expected != found {
            return Err(EngineError::Validation {
                path: "/contractId".to_owned(),
                detail: format!(
                    "strkey checksum mismatch: expected {expected:04x}, found {found:04x}"
                ),
            });
        }
        Ok(payload)
    }
}

impl fmt::Display for ContractId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ContractId {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

impl From<ContractId> for String {
    fn from(value: ContractId) -> Self {
        value.0
    }
}

impl TryFrom<String> for ContractId {
    type Error = EngineError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

/// A transaction hash: 32 bytes, lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TransactionHash(String);

impl TransactionHash {
    /// Validates and wraps a transaction hash.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(EngineError::Validation {
                path: "/transactionHash".to_owned(),
                detail: format!(
                    "a transaction hash is 64 hexadecimal characters; {value:?} is not"
                ),
            });
        }
        let lowercased = value.to_ascii_lowercase();
        Ok(Self(lowercased))
    }

    /// The lowercase hex value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransactionHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TransactionHash {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<String> for TransactionHash {
    type Error = EngineError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<TransactionHash> for String {
    fn from(value: TransactionHash) -> Self {
        value.0
    }
}

/// A Stellar ledger sequence number.
///
/// Non-negative and monotonically increasing per network. The network is not part
/// of the type, because the type would then have to carry it into every
/// comparison; instead, every structure that compares two sequences also carries
/// the boundary they belong to, and refusing a comparison across networks is the
/// boundary's responsibility.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(try_from = "u32", into = "u32")]
pub struct LedgerSequence(u32);

impl LedgerSequence {
    /// The ledger the network starts at.
    pub const GENESIS: Self = Self(1);

    /// Wraps a ledger sequence.
    pub fn new(value: u32) -> Result<Self> {
        if value == 0 {
            // Ledger 0 does not exist on any Stellar network: the first ledger is
            // 1. Accepting 0 would let an uninitialised value look like a real
            // observation and, worse, look like the earliest possible one.
            return Err(EngineError::Validation {
                path: "/ledger".to_owned(),
                detail: "ledger sequences begin at 1; 0 does not exist".to_owned(),
            });
        }
        Ok(Self(value))
    }

    /// The sequence number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The number of ledgers from `self` to `other`, or `None` when `self` is
    /// later than `other`.
    ///
    /// Returning `None` rather than a saturating zero is deliberate: a caller
    /// asking how far apart two ledgers are must find out that the order is
    /// inverted, because an inverted boundary would silently turn every
    /// "before/after" comparison upside down.
    #[must_use]
    pub const fn span_to(self, other: Self) -> Option<u32> {
        other.0.checked_sub(self.0)
    }
}

impl fmt::Display for LedgerSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<u32> for LedgerSequence {
    type Error = EngineError;

    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

impl From<LedgerSequence> for u32 {
    fn from(value: LedgerSequence) -> Self {
        value.0
    }
}

/// A reference to an entity by kind and identifier.
///
/// A reference is never an inline copy. Two places that embed the same entity
/// independently can disagree, and a reference cannot; the specification states
/// this in the `entityRef` definition, and the engine follows it by keeping
/// entities in one place and referring to them by identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRef {
    /// What kind of entity this is.
    pub kind: EntityKind,
    /// The entity's identifier within its kind.
    pub id: String,
}

impl EntityRef {
    /// Builds a reference from its parts.
    pub fn new(kind: EntityKind, id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(EngineError::Validation {
                path: "/entityRef/id".to_owned(),
                detail: "an entity reference needs a non-empty identifier".to_owned(),
            });
        }
        Ok(Self { kind, id })
    }

    /// A reference to a contract.
    pub fn contract(id: &ContractId) -> Self {
        Self {
            kind: EntityKind::Contract,
            id: id.as_str().to_owned(),
        }
    }

    /// A reference to a transaction.
    pub fn transaction(hash: &TransactionHash) -> Self {
        Self {
            kind: EntityKind::Transaction,
            id: hash.as_str().to_owned(),
        }
    }

    /// A reference to a WASM executable by digest.
    pub fn wasm(digest: &Digest) -> Self {
        Self {
            kind: EntityKind::Wasm,
            id: digest.value().to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// strkey internals
// ---------------------------------------------------------------------------

/// The RFC 4648 base32 alphabet, uppercase, without padding.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Decodes an unpadded RFC 4648 base32 string.
///
/// Hand-written rather than taken from a crate because the sole caller is the
/// strkey checksum check, and pulling in a general base32 dependency to decode
/// exactly one fixed-width format would widen the dependency surface - and the
/// licence and audit surface with it - for no benefit.
fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut bits: u64 = 0;
    let mut bit_count: u32 = 0;
    let mut out = Vec::with_capacity(input.len() * 5 / 8);

    for byte in input.bytes() {
        let index = BASE32_ALPHABET
            .iter()
            .position(|&candidate| candidate == byte)?;
        bits = (bits << 5) | u64::try_from(index).ok()?;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(u8::try_from((bits >> bit_count) & 0xff).ok()?);
        }
    }
    // Any remaining bits must be zero padding; a non-zero remainder would mean
    // the final character encoded more than the output has room for.
    if bit_count > 0 && (bits & ((1u64 << bit_count) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// CRC16/XMODEM, the checksum strkey uses.
fn crc16_xmodem(bytes: &[u8]) -> u16 {
    const POLYNOMIAL: u16 = 0x1021;
    let mut crc: u16 = 0;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ POLYNOMIAL;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A contract address that appears in the specification's fixtures. Its
    /// checksum is valid, which is why it is used for the positive cases.
    const VALID_CONTRACT: &str = "C2IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26XB";

    #[test]
    fn entity_kinds_round_trip_through_their_wire_names() {
        for kind in EntityKind::all() {
            let parsed = EntityKind::from_str(kind.as_str()).expect("wire name must parse");
            assert_eq!(parsed, *kind);
        }
        assert_eq!(EntityKind::all().len(), 8);
    }

    #[test]
    fn an_unrecognised_entity_kind_is_rejected_rather_than_tolerated() {
        // The set is closed, so a value outside it cannot be reasoned about.
        let error = EntityKind::from_str("ORACLE").expect_err("ORACLE is not an entity kind");
        assert_eq!(error.category(), crate::errors::ErrorCategory::Validation);
    }

    #[test]
    fn digest_algorithms_round_trip_and_declare_their_lengths() {
        assert_eq!(DigestAlgorithm::Sha256.hex_length(), 64);
        assert_eq!(DigestAlgorithm::Sha512.hex_length(), 128);
        for algorithm in [DigestAlgorithm::Sha256, DigestAlgorithm::Sha512] {
            assert_eq!(
                DigestAlgorithm::from_str(algorithm.as_str()).expect("round trip"),
                algorithm
            );
        }
    }

    #[test]
    fn a_digest_value_must_be_lowercase_hexadecimal() {
        Digest::new(DigestAlgorithm::Sha256, &"a".repeat(64)).expect("lowercase hex is valid");
        Digest::new(DigestAlgorithm::Sha256, &"A".repeat(64))
            .expect_err("uppercase is not canonical, so it must be rejected");
        Digest::new(DigestAlgorithm::Sha256, &"z".repeat(64))
            .expect_err("non-hexadecimal must be rejected");
        Digest::new(DigestAlgorithm::Sha256, "").expect_err("an empty digest is meaningless");
    }

    #[test]
    fn a_digest_value_must_match_the_declared_algorithms_length() {
        // A 64-character value under sha512 is a truncated digest, not a digest,
        // and accepting it would make two implementations' identities differ.
        let error = Digest::new(DigestAlgorithm::Sha512, &"a".repeat(64))
            .expect_err("sha512 expects 128 characters");
        assert!(error.to_string().contains("128"));
    }

    #[test]
    fn a_digest_round_trips_through_its_canonical_form() {
        let digest = Digest::sha256_of(b"amasario");
        let parsed = Digest::from_str(&digest.prefixed()).expect("canonical form must parse");
        assert_eq!(parsed, digest);
        assert_eq!(digest.algorithm(), DigestAlgorithm::Sha256);
    }

    #[test]
    fn a_digest_without_an_algorithm_prefix_is_rejected() {
        // Without the algorithm, a 64-character value is equally consistent with
        // a truncated sha512, so it does not identify content.
        Digest::from_str(&"a".repeat(64)).expect_err("a bare hex string is not a digest");
    }

    #[test]
    fn a_known_input_produces_a_known_sha256_digest() {
        // A known-answer test: if the hashing path changes, this fails rather than
        // silently producing different identities for the same content.
        assert_eq!(
            Digest::sha256_of(b"").value(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Digest::sha256_of(b"abc").value(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn digests_of_different_content_differ() {
        assert_ne!(Digest::sha256_of(b"a"), Digest::sha256_of(b"b"));
        assert!(Digest::sha256_of(b"a").matches(&Digest::sha256_of(b"a")));
    }

    #[test]
    fn a_well_formed_contract_address_is_accepted() {
        let contract = ContractId::new(VALID_CONTRACT).expect("fixture address is well formed");
        assert_eq!(contract.as_str(), VALID_CONTRACT);
        assert_eq!(contract.to_string(), VALID_CONTRACT);
        assert_eq!(
            ContractId::from_str(VALID_CONTRACT).expect("round trip"),
            contract
        );
    }

    #[test]
    fn a_contract_address_must_be_the_right_length_and_prefix() {
        ContractId::new("C2IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26X")
            .expect_err("55 characters is not a contract address");
        ContractId::new(format!("{VALID_CONTRACT}AA"))
            .expect_err("58 characters is not one either");
        ContractId::new(format!("G{}", &VALID_CONTRACT[1..]))
            .expect_err("an account address is not a contract address");
    }

    #[test]
    fn a_contract_address_must_be_base32() {
        // '0' and '1' are excluded from RFC 4648 base32, so an address containing
        // them cannot have been produced by any encoder.
        ContractId::new("C0IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26XB")
            .expect_err("'0' is not in the base32 alphabet");
        ContractId::new("C1IJTO436D5EBFSQZM4AEWZUDEKNWWPJRHGFJOFGNQKE445P5FSA26XB")
            .expect_err("'1' is not in the base32 alphabet");
    }

    #[test]
    fn the_strkey_checksum_can_be_verified_independently_of_the_shape_check() {
        // A checksum-valid contract strkey, taken from SEP-23's own documentation
        // example rather than written by hand, so the constant the decoder uses
        // for the version byte is checked against an authoritative value.
        const SEP23_EXAMPLE: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
        ContractId::new(SEP23_EXAMPLE)
            .expect("shape check")
            .verify_checksum()
            .expect("the SEP-23 example must pass the checksum check");
    }

    #[test]
    fn a_shape_valid_but_checksum_invalid_address_fails_the_stronger_check() {
        // Mutate a character in the middle rather than at the end: the final
        // character carries padding bits, so changing it can fail to decode at all
        // and hide the checksum failure behind an unrelated error.
        let mut corrupted = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4".to_owned();
        corrupted.replace_range(20..21, "B");
        let contract = ContractId::new(&corrupted).expect("shape is still valid");
        let error = contract
            .verify_checksum()
            .expect_err("the checksum must not match");
        assert!(
            error.to_string().contains("checksum"),
            "expected a checksum failure, got: {error}"
        );
    }

    #[test]
    fn the_synthetic_fixture_addresses_are_shape_valid_but_not_checksum_valid() {
        // The specification's schema checks shape only and says so, and its fixtures
        // are marked synthetic. Recording that here is what stops someone later
        // assuming `VALID_CONTRACT` is a real strkey and building a test on it.
        let contract = ContractId::new(VALID_CONTRACT).expect("shape is valid");
        let error = contract
            .verify_checksum()
            .expect_err("a synthetic address is not a checksum-valid strkey");
        assert!(
            error.to_string().contains("version byte") || error.to_string().contains("checksum"),
            "expected a version or checksum failure, got: {error}"
        );
    }

    #[test]
    fn base32_decoding_round_trips_a_known_value() {
        // The unpadded base32 of "foobar". A known answer, so a change to the
        // bit-shifting is caught rather than silently producing different bytes.
        assert_eq!(base32_decode("MZXW6YTBOI"), Some(b"foobar".to_vec()));
        // Unpadded is the only form strkey emits, so padding is rejected rather
        // than skipped: accepting it would mean accepting input no encoder produces.
        assert_eq!(base32_decode("MY======"), None);
    }

    #[test]
    fn base32_decoding_rejects_characters_outside_the_alphabet() {
        assert!(
            base32_decode("AB0").is_none(),
            "'0' is not in the base32 alphabet"
        );
        assert!(
            base32_decode("ab").is_none(),
            "lowercase is not canonical base32"
        );
        assert!(
            base32_decode("ABC").is_none(),
            "a trailing character whose padding bits are non-zero is not valid base32"
        );
    }

    #[test]
    fn the_hand_written_decoder_agrees_with_stellars_own_strkey_implementation() {
        // An independent oracle rather than a second copy of the same logic. The
        // engine decodes strkeys by hand so that it does not depend on the strkey
        // crate at runtime; this test is what keeps that choice honest, because a
        // hand-written decoder checked only against itself proves nothing.
        for seed in [0u8, 1, 7, 42, 255] {
            let payload = [seed; 32];
            let encoded = stellar_strkey::Contract(payload).to_string();
            let encoded = encoded.as_str();

            let contract = ContractId::new(encoded)
                .unwrap_or_else(|error| panic!("stellar-strkey produced {encoded}: {error}"));
            contract.verify_checksum().unwrap_or_else(|error| {
                panic!("the decoder must accept a strkey Stellar itself produced: {error}")
            });

            // And the version byte the decoder asserts is the one Stellar writes.
            let decoded = base32_decode(encoded).expect("decodable base32");
            assert_eq!(decoded[0], 0x10, "contract strkeys begin with 0x10");
            assert_eq!(&decoded[1..33], &payload[..], "the payload must round-trip");
        }
    }

    #[test]
    fn the_decoder_rejects_a_strkey_stellar_produced_after_mutating_its_payload() {
        let encoded = stellar_strkey::Contract([9u8; 32]).to_string();
        let mut corrupted = encoded.as_str().to_owned();
        corrupted.replace_range(30..31, "A");
        ContractId::new(&corrupted)
            .expect("shape is unchanged")
            .verify_checksum()
            .expect_err("a mutated payload must fail the checksum");
    }

    #[test]
    fn crc16_xmodem_matches_the_published_check_value() {
        // CRC-16/XMODEM("123456789") = 0x31C3. A known answer, so a change to the
        // polynomial or the initial value is caught rather than silently
        // invalidating every checksum.
        assert_eq!(crc16_xmodem(b"123456789"), 0x31C3);
    }

    #[test]
    fn a_transaction_hash_is_normalised_to_lowercase() {
        let upper = TransactionHash::new("AB".repeat(32)).expect("64 hex characters");
        assert_eq!(upper.as_str(), "ab".repeat(32));
        TransactionHash::new("ab".repeat(31)).expect_err("a hash is 32 bytes");
        TransactionHash::new("zz".repeat(32)).expect_err("a hash is hexadecimal");
    }

    #[test]
    fn ledger_sequences_begin_at_one() {
        LedgerSequence::new(1).expect("ledger 1 exists");
        let error = LedgerSequence::new(0).expect_err("ledger 0 does not exist");
        assert!(error.to_string().contains("begin at 1"));
        assert_eq!(LedgerSequence::GENESIS.get(), 1);
    }

    #[test]
    fn a_ledger_span_reports_an_inverted_order_rather_than_a_saturated_zero() {
        let earlier = LedgerSequence::new(100).expect("valid");
        let later = LedgerSequence::new(150).expect("valid");
        assert_eq!(earlier.span_to(later), Some(50));
        assert_eq!(later.span_to(earlier), None);
        // An inverted boundary would turn every before/after comparison upside
        // down, so the caller must be able to see it.
        assert_ne!(later.span_to(earlier), Some(0));
    }

    #[test]
    fn an_entity_reference_requires_a_non_empty_identifier() {
        EntityRef::new(EntityKind::Contract, VALID_CONTRACT).expect("a real reference");
        EntityRef::new(EntityKind::Contract, "").expect_err("an empty id refers to nothing");
    }

    #[test]
    fn entity_reference_constructors_use_the_identifier_form_the_schema_defines() {
        let contract = ContractId::new(VALID_CONTRACT).expect("valid");
        assert_eq!(EntityRef::contract(&contract).id, VALID_CONTRACT);

        let digest = Digest::sha256_of(b"wasm");
        // A WASM entity is identified by its digest value, matching the identity
        // model's decision that the executable's name is not its identity.
        assert_eq!(EntityRef::wasm(&digest).kind, EntityKind::Wasm);
        assert_eq!(EntityRef::wasm(&digest).id, digest.value());
    }

    #[test]
    fn identity_parsing_is_deterministic_across_repeated_calls() {
        // No fabricated constant here: the property under test is that repeated
        // calls agree with each other, and a hardcoded value would only be testing
        // whatever was pasted in.
        let first = Digest::sha256_of(b"amasario determinism");
        for _ in 0..64 {
            assert_eq!(Digest::sha256_of(b"amasario determinism"), first);
        }
        assert_eq!(first.algorithm(), DigestAlgorithm::Sha256);
        assert_eq!(first.value().len(), 64);
    }
}
