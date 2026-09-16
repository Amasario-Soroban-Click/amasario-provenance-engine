//! Fuzzes the module decoder, the digest, and the identity types built from strings.
//!
//! # Why this is the target that matters most
//!
//! Every provenance claim the engine makes ultimately rests on a digest over bytes that
//! arrived from a network endpoint. The module bytes are the only attacker-influenced
//! input in the whole pipeline: the dependency graph, the impact surface and the report
//! are all derived from the engine's own types, but the module is whatever the endpoint
//! returned, and the decoder is the code that has to survive it.
//!
//! # What is asserted, beyond "it did not panic"
//!
//! A fuzz target that only checks for a panic finds a crash and nothing else. The
//! assertions below are the engine's own documented contracts, checked on every input:
//!
//! * decoding is total - any byte string is either a module or a refusal, never a panic;
//! * a decoded module satisfies the cheap pre-check, because a decoder that accepted
//!   something `looks_like_module` rejects would make the pre-check a lie and would mean
//!   the decoder had stopped checking the header;
//! * the digest is a function - two calls over the same bytes agree, because a digest
//!   that changed between calls would make every verification a coin toss;
//! * verifying a module against its own digest succeeds, which is the one case the whole
//!   artefact-identity model depends on.
//!
//! # The string path
//!
//! `Digest::new` and `TransactionHash::new` take strings that came, in a real run, from
//! a recorded response. They are fuzzed as well: a constructor that panicked on a
//! malformed value would turn a malformed response into a crash rather than into the
//! refusal the error model exists to produce.

#![no_main]

use libfuzzer_sys::fuzz_target;

use amasario_contract::wasm;
use amasario_core::{Digest, DigestAlgorithm, TransactionHash};

fuzz_target!(|data: &[u8]| {
    // -- the module decoder, which is the untrusted path --------------------
    let decoded = wasm::parse_module(data);
    let looks_like_a_module = wasm::looks_like_module(data);

    if decoded.is_ok() {
        assert!(
            looks_like_a_module,
            "a decoder accepted bytes the cheap pre-check rejects, so the pre-check no \
             longer describes what the decoder accepts"
        );
    }

    // -- the digest, which every identity is built from ---------------------
    let digest = wasm::digest_of(data);
    assert_eq!(
        digest,
        wasm::digest_of(data),
        "the digest is not a function of its input"
    );
    assert!(
        wasm::verify_digest(&digest, data).is_ok(),
        "a module does not verify against its own digest"
    );

    // A recorded digest from elsewhere must be refused, not trusted. The engine cannot
    // tell an honest digest from a forged one, which is why it recomputes; this asserts
    // that the comparison is real rather than a constant.
    let forged = Digest::new(DigestAlgorithm::Sha256, &"00".repeat(32)).expect("a syntactically valid digest");
    if forged != digest {
        assert!(
            wasm::verify_digest(&forged, data).is_err(),
            "a digest that does not describe the bytes was accepted"
        );
    }

    // -- the identity constructors, which read recorded strings -------------
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = Digest::new(DigestAlgorithm::Sha256, text);
        let _ = TransactionHash::new(text.to_owned());
    }

    // A long identifier is the case a fixed-size buffer would overrun; building one
    // from every input costs nothing and exercises the length check.
    let _ = TransactionHash::new("a".repeat(data.len().min(4096)));

    // -- the provenance documents, which a consumer may hand the engine ------
    //
    // `deny_unknown_fields` means most inputs are refused, which is the expected and
    // correct outcome; the assertion is that a refusal is what happens rather than a
    // panic inside a derived deserialiser.
    let _: Result<amasario_provenance::attestations::Attestation, _> =
        serde_json::from_slice(data);
});
