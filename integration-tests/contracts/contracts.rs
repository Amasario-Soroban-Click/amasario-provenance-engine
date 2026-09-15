//! Contract identity and WebAssembly decoding, checked against the corpus.
//!
//! # The inference this suite refuses to let the engine make
//!
//! A WebAssembly module that exports a function does not thereby declare a contract
//! interface. In Soroban, a contract's interface comes from the `contractspecv0` custom
//! section, and a module's ordinary exports are implementation detail. A tool that
//! listed exports as the contract's functions would be inventing an interface out of
//! bytes that do not contain one - and the fixture that catches that is a module which
//! really does export `greet` and really does have no spec section.
//!
//! # Why the digest is recomputed
//!
//! The WASM fixtures store their bytes as hex *and* the digest computed over them. If
//! only the digest were stored, a fixture could not be checked; if only the bytes were,
//! nothing would tie the fixture to an identity. Storing both means the suite can
//! recompute the digest from the bytes and compare, which is the same computation a
//! provenance verification performs.

use amasario_contract::wasm::{CONTRACT_SPEC_SECTION, looks_like_module, parse_module};
use amasario_contract::{ContractExecutableKind, ContractIdentity};
use amasario_core::ErrorCategory;
use amasario_integration_tests::corpus::{Corpus, assert_committed, parse_committed};
use amasario_integration_tests::documents;
use amasario_integration_tests::recordings;
use amasario_integration_tests::wasm_modules;
use amasario_network::{classify_code, classify_status, status_is_absent, status_is_transient};
use serde_json::Value;

/// One WASM fixture, as it is committed.
fn module_fixture(name: &str) -> Value {
    parse_committed("wasm", &format!("{name}.json"))
}

/// Every WASM fixture is committed, and what is committed is what the corpus defines.
#[test]
fn every_module_fixture_matches_the_corpus() {
    for spec in wasm_modules::all() {
        let bytes = (spec.bytes)();
        let digest = amasario_contract::wasm::digest_of(&bytes);
        let expected = serde_json::json!({
            "name": spec.name,
            "note": spec.note,
            "hex": wasm_modules::hex_of(&bytes),
            "byteSize": bytes.len(),
            "digest": digest.value(),
            "digestAlgorithm": "sha256",
            "magicPresent": spec.magic_present,
            "isAModule": spec.is_a_module,
        });
        let mut text = serde_json::to_string_pretty(&expected).expect("a value serialises");
        text.push('\n');
        assert_committed("wasm", &format!("{}.json", spec.name), &text);
    }
}

/// The recorded digest is the digest of the recorded bytes.
///
/// The assertion that ties a fixture to an identity: a fixture that stored a digest its
/// bytes do not produce would be a fixture claiming a provenance that is false, which is
/// precisely the class of error this repository exists to detect in others.
#[test]
fn every_recorded_digest_is_the_digest_of_the_recorded_bytes() {
    for spec in wasm_modules::all() {
        let fixture = module_fixture(spec.name);
        let hex = fixture["hex"].as_str().expect("a hex string");
        let bytes = hex::decode(hex).expect("the recorded bytes decode");
        assert_eq!(
            bytes.len(),
            usize::try_from(fixture["byteSize"].as_u64().expect("a size")).expect("fits"),
            "{}: the recorded size disagrees with the bytes",
            spec.name
        );
        assert_eq!(
            fixture["digest"].as_str().expect("a digest"),
            amasario_contract::wasm::digest_of(&bytes).value(),
            "{}: the recorded digest is not the digest of the recorded bytes",
            spec.name
        );
    }
}

/// The magic check is a magic check, and the decoder is what validates the version.
///
/// The distinction an earlier version of this corpus got wrong by treating the two as
/// one. `looks_like_module` checks four bytes and says so; a module with a truncated
/// header or a wrong version word passes it and must still be refused by `parse_module`.
#[test]
fn the_magic_check_is_weaker_than_the_decoder_and_each_fixture_shows_it() {
    for spec in wasm_modules::all() {
        let fixture = module_fixture(spec.name);
        let bytes = hex::decode(fixture["hex"].as_str().expect("hex")).expect("decodes");
        assert_eq!(
            looks_like_module(&bytes),
            spec.magic_present,
            "{}: magic-number disagreement",
            spec.name
        );
        assert_eq!(
            fixture["magicPresent"].as_bool().expect("a flag"),
            spec.magic_present
        );
    }

    // The two fixtures that exist to show the gap.
    assert!(looks_like_module(&wasm_modules::truncated_header()));
    assert!(parse_module(&wasm_modules::truncated_header()).is_err());
    assert!(looks_like_module(&wasm_modules::wrong_version()));
    assert!(parse_module(&wasm_modules::wrong_version()).is_err());
}

/// A module decodes iff the fixture says it does, and a non-module is refused.
#[test]
fn only_the_fixtures_that_are_modules_decode() {
    for spec in wasm_modules::all() {
        let fixture = module_fixture(spec.name);
        let bytes = hex::decode(fixture["hex"].as_str().expect("hex")).expect("decodes");
        let decoded = parse_module(&bytes);

        assert_eq!(
            decoded.is_ok(),
            spec.is_a_module,
            "{}: decodability disagrees with the fixture's claim ({:?})",
            spec.name,
            decoded.err()
        );
        assert_eq!(
            fixture["isAModule"].as_bool().expect("a flag"),
            spec.is_a_module
        );

        if let Err(error) = decoded {
            assert!(
                !error.to_string().is_empty(),
                "{}: a refusal must explain itself",
                spec.name
            );
        }
    }
}

/// A module with an export and no contract spec section reports no interface.
///
/// The single most important assertion in this file, and the reason the fixture is a
/// hand-encoded module rather than a recorded contract.
#[test]
fn a_module_without_a_contract_spec_section_reports_no_interface() {
    let bytes = wasm_modules::module_with_export_only();
    let module = parse_module(&bytes).expect("the module decodes");

    // The export is real: the fixture is not trivially empty.
    assert!(
        module.exports.iter().any(|export| export.name == "greet"),
        "the fixture must actually export `greet`, or the assertion below is vacuous: {:?}",
        module
            .exports
            .iter()
            .map(|export| export.name.as_str())
            .collect::<Vec<_>>()
    );

    // And no section declares a contract interface, so none may be reported.
    let has_spec = module
        .sections
        .iter()
        .any(|section| section.name.as_deref() == Some(CONTRACT_SPEC_SECTION));
    assert!(
        !has_spec,
        "the fixture deliberately has no {CONTRACT_SPEC_SECTION} section"
    );
}

/// The empty and custom-section modules decode without inventing sections.
#[test]
fn a_module_with_no_sections_decodes_to_an_empty_module() {
    let empty = parse_module(&wasm_modules::empty_module()).expect("an empty module decodes");
    assert!(
        empty.sections.is_empty(),
        "an empty module has no sections: {:?}",
        empty.sections
    );
    assert!(empty.exports.is_empty());
    assert!(empty.imports.is_empty());

    let custom = parse_module(&wasm_modules::custom_section_module())
        .expect("a module with one custom section decodes");
    assert_eq!(custom.sections.len(), 1);
    assert_eq!(custom.sections[0].name.as_deref(), Some("amasario-fixture"));
    assert!(
        custom.exports.is_empty(),
        "a custom section declares no exports"
    );
}

/// Every contract identity fixture parses and satisfies its own validation.
#[test]
fn every_contract_identity_fixture_is_valid() {
    for (name, _) in documents::contract_identities() {
        let file = format!("{name}.json");
        let identity: ContractIdentity = parse_committed("contracts", &file);
        identity
            .validate()
            .unwrap_or_else(|error| panic!("{file}: {error}"));
    }
}

/// A WASM contract's identity carries its module digest; an asset contract's does not.
///
/// The specification's rule: a contract identity whose executable kind disagrees with
/// its digest claims a module that is not there, or omits one that is.
#[test]
fn an_identity_records_a_module_exactly_when_it_has_one() {
    for (name, _) in documents::contract_identities() {
        let file = format!("{name}.json");
        let identity: ContractIdentity = parse_committed("contracts", &file);
        match identity.executable_kind {
            ContractExecutableKind::Wasm => {
                let digest = identity.require_wasm_hash().unwrap_or_else(|error| {
                    panic!("{file}: a WASM identity needs a digest: {error}")
                });
                assert!(!digest.value().is_empty(), "{file}");
            },
            ContractExecutableKind::StellarAsset => assert!(
                identity.wasm_hash.is_none(),
                "{file}: an asset contract has no deployed module"
            ),
        }
    }
}

/// The upgraded pair records two modules at two ledgers for one address.
#[test]
fn the_upgrade_fixture_records_a_new_module_at_a_later_ledger() {
    let before: ContractIdentity = parse_committed("contracts", "wasm-contract.json");
    let after: ContractIdentity = parse_committed("contracts", "wasm-contract-after-upgrade.json");

    assert_eq!(
        before.contract_id, after.contract_id,
        "an upgrade is the same address running a different module"
    );
    assert_ne!(
        before.wasm_hash, after.wasm_hash,
        "the upgrade must record a different module, or the pair demonstrates nothing"
    );
    assert!(
        after.is_upgraded_relative_to(&before),
        "the later identity must be reported as an upgrade of the earlier one"
    );
    assert!(
        before.resolved_at_ledger.get() < after.resolved_at_ledger.get(),
        "the upgrade happened later"
    );
    assert!(
        !before.is_same_identity_as(&after),
        "two different module digests are not one identity"
    );
}

/// An identity's fingerprint is stable, and the boundary is not part of it.
///
/// The distinction the specification draws between what identifies a contract and what
/// merely qualifies an observation of it: an upgrade changes the digest but not the
/// address, so two observations of one address share a fingerprint.
#[test]
fn the_fingerprint_is_stable_and_excludes_when_it_was_observed() {
    let identity: ContractIdentity = parse_committed("contracts", "wasm-contract.json");
    assert_eq!(identity.fingerprint(), identity.fingerprint());
    assert!(!identity.fingerprint().value().is_empty());
    assert!(
        identity.network_qualified_id().contains("testnet"),
        "a fingerprint is qualified by its chain, because one address exists on every chain"
    );
}

/// A recorded empty result and a recorded error are classified differently.
#[test]
fn an_empty_result_is_an_absence_and_an_unimplemented_method_is_a_failure() {
    // The absence: a successful response with no entries.
    let empty = recordings::RPC_EMPTY_ENTRIES.value();
    assert!(
        empty["result"]["entries"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the recording must be an empty successful result"
    );
    assert!(
        empty.get("error").is_none(),
        "an absence is not reported as an error"
    );

    // The failure: a method the endpoint does not implement.
    let method_not_found = recordings::RPC_ERROR_METHOD_NOT_FOUND.value();
    let code =
        i32::try_from(method_not_found["error"]["code"].as_i64().expect("a code")).expect("fits");
    let message = method_not_found["error"]["message"]
        .as_str()
        .expect("a message");
    let error = classify_code("https://rpc.testnet", code, message);
    assert_eq!(
        error.category(),
        ErrorCategory::Network,
        "an unimplemented method is a network failure: {error}"
    );
    assert!(
        !error.to_string().is_empty(),
        "the classification must explain itself"
    );
}

/// A truncated body is a defect rather than something to retry.
#[test]
fn a_malformed_recording_is_classified_as_a_defect() {
    let body = recordings::MALFORMED_BODY.body;
    assert!(
        serde_json::from_str::<Value>(body).is_err(),
        "the recording exists to be unparseable"
    );
    assert!(!recordings::MALFORMED_BODY.is_json());
}

/// The HTTP status vocabulary keeps absence, transience and rejection apart.
#[test]
fn every_recorded_status_is_classified_as_the_corpus_says() {
    for recorded in recordings::statuses() {
        assert_eq!(
            status_is_absent(recorded.status),
            recorded.absent,
            "HTTP {}: absent",
            recorded.status
        );
        assert_eq!(
            status_is_transient(recorded.status),
            recorded.transient,
            "HTTP {}: transient",
            recorded.status
        );

        let error = classify_status(
            "https://horizon-testnet.stellar.org",
            recorded.status,
            recorded.detail,
        );
        assert_eq!(error.category(), ErrorCategory::Network);
        assert!(
            error.to_string().contains(&recorded.status.to_string()),
            "the classification must name the status: {error}"
        );
    }

    // The distinction the corpus is built to protect: a `404` is an absence and is not
    // worth retrying, and a `503` is worth retrying and is not an absence. A tool that
    // merged them would retry forever or report a busy server as a missing contract.
    assert!(status_is_absent(404) && !status_is_transient(404));
    assert!(status_is_transient(503) && !status_is_absent(503));
}

/// Every contract fixture is the document the builder produces.
#[test]
fn every_contract_fixture_is_current() {
    for (name, identity) in documents::contract_identities() {
        assert_committed(
            "contracts",
            &format!("{name}.json"),
            &documents::rendered(&identity),
        );
    }
}

/// The corpus directory holds exactly the fixture set the corpus defines.
#[test]
fn the_corpus_holds_no_fixture_the_corpus_does_not_define() {
    let mut expected: Vec<String> = documents::contract_identities()
        .into_iter()
        .map(|(name, _)| format!("{name}.json"))
        .collect();
    expected.extend(
        recordings::all()
            .into_iter()
            .filter(|recording| recording.directory == "contracts")
            .map(|recording| recording.file.to_owned()),
    );
    expected.sort();

    let actual: Vec<String> = Corpus::files_in("contracts")
        .into_iter()
        .map(|path| {
            path.file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(actual, expected);
}
