//! Cross-checks the engine's hand-written contract-strkey decoder against
//! Stellar's own implementation.
//!
//! `amasario-core` decodes contract strkeys by hand rather than depending on
//! `stellar-strkey` at runtime, so the decoder needs an oracle that is not itself.
//! `stellar-strkey` is the Stellar organisation's implementation and is a
//! dev-dependency for exactly this purpose: a decoder tested only against its own
//! fixtures proves that it is self-consistent, not that it is correct.
//!
//! What is established here:
//!
//! * every address the oracle produces is accepted by the engine's shape check and
//!   passes its checksum verification;
//! * the 32 bytes the engine recovers are the bytes the oracle encoded, so the
//!   engine queries the contract the address names rather than a neighbour;
//! * a corrupted address is rejected - which matters because a corrupted address
//!   decodes to a *different* contract, and the network answer for that contract is
//!   "no such entry", indistinguishable from a contract that does not exist;
//! * the kind identification agrees, so an account address is not mistaken for a
//!   contract.

use amasario_core::ContractId;

/// Payloads chosen so that a transcription or bit-order error has somewhere to hide.
///
/// An all-zero payload hides byte-order mistakes because every byte is equal. The
/// gradient does not: reversing the byte order, or reversing the bits within a
/// byte, changes it.
const ALL_ZERO: [u8; 32] = [0x00; 32];
const ALL_ONES: [u8; 32] = [0xff; 32];
const GRADIENT: [u8; 32] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00,
];

/// Encodes a payload with Stellar's implementation.
fn oracle_encode(payload: [u8; 32]) -> String {
    // `format!` rather than `to_string`, because the crate's inherent `to_string`
    // returns a fixed-capacity string rather than an owned one.
    format!("{}", stellar_strkey::Contract(payload))
}

#[test]
fn every_address_the_oracle_produces_is_accepted_and_yields_the_same_bytes() {
    for payload in [ALL_ZERO, ALL_ONES, GRADIENT] {
        let encoded = oracle_encode(payload);

        // The shape check accepts it.
        let contract = ContractId::new(encoded.clone())
            .unwrap_or_else(|error| panic!("the engine rejected {encoded}: {error}"));

        // The checksum check accepts it.
        contract.verify_checksum().unwrap_or_else(|error| {
            panic!("the engine rejected the checksum of {encoded}: {error}")
        });

        // The recovered identifier is the one the oracle encoded. This is the
        // assertion that keeps the engine from querying a different contract than
        // the one the address names.
        assert_eq!(
            contract.payload().expect("the payload decodes"),
            payload,
            "the engine recovered different bytes than the oracle encoded for {encoded}"
        );
    }
}

#[test]
fn the_oracle_and_the_engine_agree_on_the_kind_of_the_address() {
    // Kind identification is separate from decoding: an account strkey is also
    // 56 base32 characters, and misidentifying one as a contract would produce a
    // request for a contract that cannot exist.
    let contract = oracle_encode(GRADIENT);
    assert!(
        matches!(
            contract.parse::<stellar_strkey::Strkey>(),
            Ok(stellar_strkey::Strkey::Contract(_))
        ),
        "the oracle must identify {contract} as a contract"
    );
    assert!(contract.starts_with('C'));

    let account = format!("{}", stellar_strkey::ed25519::PublicKey([0x11; 32]));
    assert!(
        matches!(
            account.parse::<stellar_strkey::Strkey>(),
            Ok(stellar_strkey::Strkey::PublicKeyEd25519(_))
        ),
        "the oracle must identify {account} as an account"
    );
    // The engine's shape check rejects it, since it requires a leading `C`.
    ContractId::new(account).expect_err("an account address is not a contract address");
}

#[test]
fn a_corrupted_address_is_rejected_by_its_checksum() {
    // This is the failure the checksum exists to catch. A single altered character
    // decodes to a different set of bytes, so the engine would query a contract the
    // user did not name and receive an absence - which reads as a finding that the
    // contract does not exist.
    let mut corrupted = oracle_encode(GRADIENT);
    let last = corrupted.pop().expect("the encoding is not empty");
    corrupted.push(if last == 'A' { 'B' } else { 'A' });

    let contract = ContractId::new(corrupted.clone())
        .expect("the shape of a corrupted address is still valid");
    let error = contract
        .verify_checksum()
        .expect_err("the checksum must not verify");
    assert!(
        error.to_string().contains("checksum"),
        "the failure must name the checksum: {error}"
    );
}

#[test]
fn decoding_a_corrupted_address_is_refused_rather_than_returning_neighbouring_bytes() {
    // `payload` verifies before it decodes, so a caller cannot obtain an identifier
    // that no checksum ever endorsed.
    let mut corrupted = oracle_encode(GRADIENT);
    let last = corrupted.pop().expect("the encoding is not empty");
    corrupted.push(if last == 'A' { 'B' } else { 'A' });

    let contract = ContractId::new(corrupted).expect("the shape is still valid");
    assert!(contract.payload().is_err());
}

#[test]
fn an_address_of_the_wrong_length_is_rejected() {
    let encoded = oracle_encode(GRADIENT);
    let truncated = &encoded[..encoded.len() - 1];
    ContractId::new(truncated).expect_err("a truncated address is refused");

    let extended = format!("{encoded}A");
    ContractId::new(extended).expect_err("an over-long address is refused");
}

#[test]
fn a_non_base32_character_is_rejected() {
    // `0`, `1`, `8` and `9` are not in the RFC 4648 base32 alphabet, so an address
    // containing one cannot have come from a correct encoding.
    for replacement in ['0', '1', '8', '9'] {
        let mut encoded = oracle_encode(GRADIENT);
        encoded.pop();
        encoded.push(replacement);
        ContractId::new(encoded.clone())
            .expect_err("a character outside the base32 alphabet must be refused at construction");
    }
}
