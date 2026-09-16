//! The first-party reference contract, asserted against the real SDK's output.
//!
//! # What this suite is for
//!
//! Everything else in the corpus is either a hand-assembled module or a borrowed
//! testnet contract. This is the only place where the engine's module analysis is
//! checked against bytes this project built from its own source, with the toolchain
//! pinned, and where the expected answer is written down.
//!
//! That matters because these bytes are what a real contract looks like, and they do
//! not look like what a reader of the engine's source might assume. The findings below
//! are the reason the suite exists; each one is a fact about the toolchain that nothing
//! in the corpus previously recorded.
//!
//! # What a real `soroban-sdk` 27 module actually contains
//!
//! * Eleven sections, ids 1 through 11 with 7, 8 and 12 absent, followed by six custom
//!   sections: `contractspecv0`, `contractenvmetav0`, `contractmetav0`, `name`,
//!   `producers` and `target_features`. Nothing above id 11, so the engine's
//!   [`MAX_KNOWN_SECTION_ID`] boundary is not reached by real output today.
//! * An interface that decodes completely, with no undecodable trailing bytes, and with
//!   the doc comments written in the Rust source carried into the section.
//! * **Host functions that are not imported from a namespace called `env`.** They are
//!   one- and two-character names, and `soroban-env-macros` generates them by scheme
//!   rather than declaring them: `_`, then `0`-`9`, `a`-`z`, `A`-`Z`, then the
//!   cartesian product, which the crate's own comment says covers "4032 functions per
//!   module". The matching module name (`v`, `b`, `a`, `x`, `i`, `l`, `d` here) is
//!   generated the same way.
//!
//! [`MAX_KNOWN_SECTION_ID`]: amasario_contract::wasm::MAX_KNOWN_SECTION_ID

use amasario_contract::interface::decode_spec_section;
use amasario_contract::wasm::{
    CONTRACT_ENV_META_SECTION, CONTRACT_SPEC_SECTION, MAX_KNOWN_SECTION_ID, SectionKind, digest_of,
    looks_like_module, parse_module,
};
use amasario_integration_tests::reference_contract::{
    CALLEE_DIGEST, CALLEE_FILE, CALLEE_FUNCTIONS, CALLER_DIGEST, CALLER_FILE, CALLER_FUNCTIONS,
    ExpectedFunction, SDK_CUSTOM_SECTIONS, SDK_EXPORTS, bytes, digest_value, provenance,
};

/// Each module is exactly what its provenance record says it is.
///
/// The records are transcribed rather than generated, so this is what makes them a
/// record rather than a claim: the record names this module, the module's bytes hash to
/// the digest the record states, its length is the recorded length, the directory it
/// says it was built from exists, and the toolchain is stated rather than left blank. A
/// rebuild that changed either contract without updating its record fails here rather
/// than leaving a record that describes a module no longer present.
#[test]
fn each_module_matches_its_provenance_record() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the integration crate sits in the repository root")
        .to_path_buf();

    for (module_file, recorded) in [(CALLEE_FILE, CALLEE_DIGEST), (CALLER_FILE, CALLER_DIGEST)] {
        let record = provenance(module_file);

        assert_eq!(
            record.module_file, module_file,
            "the record beside {module_file} names a different module"
        );
        assert_eq!(record.name, module_file.trim_end_matches(".wasm"));
        assert_eq!(
            record.digest, recorded,
            "{module_file}: the provenance record and this suite disagree on the digest"
        );
        assert_eq!(
            digest_value(module_file),
            record.digest,
            "{module_file} does not hash to the digest its provenance record states"
        );
        assert_eq!(
            bytes(module_file).len(),
            record.byte_size,
            "{module_file}: the provenance record states the wrong size"
        );
        assert_eq!(record.digest_algorithm, "sha256");
        assert!(
            repository.join(&record.builds_from).is_dir(),
            "{module_file} claims to be built from `{}`, which does not exist",
            record.builds_from
        );
        assert!(
            !record.toolchain.soroban_sdk.is_empty()
                && !record.toolchain.rustc.is_empty()
                && !record.toolchain.target.is_empty(),
            "{module_file}'s provenance record does not state the toolchain that built it"
        );
    }
}

/// Both modules are genuine WebAssembly modules, and hash to what the record says.
///
/// The digest is the engine's own computation, so this asserts the identity path
/// against a real artefact rather than against a synthesised one.
#[test]
fn both_reference_modules_are_modules_and_match_their_recorded_digest() {
    for file in [CALLEE_FILE, CALLER_FILE] {
        let raw = bytes(file);
        assert!(
            looks_like_module(&raw),
            "{file} is not recognised as a WebAssembly module"
        );
        assert_eq!(
            parse_module(&raw)
                .unwrap_or_else(|error| panic!("{file} does not parse: {error}"))
                .module_digest
                .value(),
            digest_value(file),
            "{file}: the parsed module's digest is not the digest of its bytes"
        );
    }

    assert_eq!(digest_value(CALLEE_FILE), CALLEE_DIGEST);
    assert_eq!(digest_value(CALLER_FILE), CALLER_DIGEST);
    assert_eq!(digest_of(&bytes(CALLEE_FILE)).value(), CALLEE_DIGEST);
}

/// A real module declares its interface, and declares it exactly once.
///
/// The engine reports the interface as absent when `contractspecv0` is missing, and a
/// duplicate section is a malformed module. Real output is the case that has to read as
/// present-and-unambiguous.
#[test]
fn a_real_module_declares_its_interface_exactly_once() {
    for file in [CALLEE_FILE, CALLER_FILE] {
        let module = parse_module(&bytes(file)).expect("a real module parses");

        assert!(
            module.declares_interface(),
            "{file}: a contract built by the SDK must declare an interface"
        );
        assert_eq!(
            module.sections_of(SectionKind::Custom).len(),
            SDK_CUSTOM_SECTIONS.len(),
            "{file}: the SDK's custom section set has changed, so the readers need \
             reviewing"
        );
        assert_eq!(
            module
                .sections
                .iter()
                .filter(|section| section.name.as_deref() == Some(CONTRACT_SPEC_SECTION))
                .count(),
            1,
            "{file}: `{CONTRACT_SPEC_SECTION}` must appear exactly once"
        );
        assert_eq!(
            module
                .sections
                .iter()
                .filter(|section| section.name.as_deref() == Some(CONTRACT_ENV_META_SECTION))
                .count(),
            1,
            "{file}: `{CONTRACT_ENV_META_SECTION}` must appear exactly once"
        );
    }
}

/// Every custom section the SDK 27 toolchain writes is present.
///
/// Named individually rather than counted, because a count passes when one section is
/// swapped for another and the engine's reader for the missing one goes untested.
#[test]
fn a_real_module_carries_every_custom_section_the_sdk_writes() {
    for file in [CALLEE_FILE, CALLER_FILE] {
        let module = parse_module(&bytes(file)).expect("a real module parses");
        let present: Vec<&str> = module
            .sections
            .iter()
            .filter_map(|section| section.name.as_deref())
            .collect();

        for expected in SDK_CUSTOM_SECTIONS {
            assert!(
                present.contains(expected),
                "{file}: `{expected}` is absent, but the pinned SDK writes it. Present: \
                 {present:?}"
            );
        }
    }
}

/// No section of a real module is above the engine's known-section boundary.
///
/// The engine refuses a module containing a section id above
/// [`MAX_KNOWN_SECTION_ID`], on the grounds that skipping a section it cannot
/// interpret would mean attesting to a module it only partly read. That refusal is
/// correct but would become a cliff if real output ever carried an id above the bound,
/// so this asserts the headroom is real: real SDK 27 modules stop at id 11.
#[test]
fn no_real_section_reaches_the_unknown_section_boundary() {
    for file in [CALLEE_FILE, CALLER_FILE] {
        let module = parse_module(&bytes(file)).expect("a real module parses");
        let highest = module
            .sections
            .iter()
            .map(|section| section.kind.id())
            .max()
            .expect("a module has sections");

        assert!(
            highest < MAX_KNOWN_SECTION_ID,
            "{file}: a section id of {highest} is at or above the engine's boundary of \
             {MAX_KNOWN_SECTION_ID}. Either the SDK has moved ahead of the engine, or \
             the boundary needs raising - and until it is, this module is refused"
        );
    }
}

/// The callee's decoded interface is exactly what its source declares.
///
/// The whole interface, not a sample: a decoder that dropped a trailing function, or
/// invented one, would pass a membership check and fail here.
#[test]
fn the_callee_interface_decodes_to_exactly_the_declared_functions() {
    assert_interface(CALLEE_FILE, CALLEE_FUNCTIONS);
}

/// The caller's decoded interface is exactly what its source declares.
#[test]
fn the_caller_interface_decodes_to_exactly_the_declared_functions() {
    assert_interface(CALLER_FILE, CALLER_FUNCTIONS);
}

/// The caller does not re-export the callee's interface.
///
/// Worth an assertion of its own because the intuitive expectation is the opposite:
/// the caller holds a typed client generated from the callee's specification, so it is
/// tempting to expect the callee's functions to appear among the caller's. They do not.
/// A tool that merged an imported specification into its importer's interface would
/// attribute three functions to a contract that does not declare them, which is exactly
/// the class of invention this corpus exists to catch.
#[test]
fn the_caller_does_not_republish_the_callee_interface() {
    let module = parse_module(&bytes(CALLER_FILE)).expect("a real module parses");
    let interface = decode_spec_section(module.spec_section.as_deref().expect("a spec section"))
        .expect("decodes");

    for function in CALLEE_FUNCTIONS {
        assert!(
            interface.function(function.name).is_none(),
            "the caller declares `{}`, which belongs to the callee. The dependency is a \
             fact about invocation, not a licence to merge the two interfaces",
            function.name
        );
    }
}

/// The callee's storage key decodes as the union its source declares.
///
/// Reaches the type decoding rather than only the function decoding: every parameter
/// above is a scalar, and this is the only non-scalar type either contract declares.
#[test]
fn the_callee_storage_key_decodes_as_its_declared_union() {
    let module = parse_module(&bytes(CALLEE_FILE)).expect("a real module parses");
    let interface = decode_spec_section(module.spec_section.as_deref().expect("a spec section"))
        .expect("decodes");

    let union = interface
        .unions
        .iter()
        .find(|union| union.name == "DataKey")
        .unwrap_or_else(|| {
            panic!(
                "the callee declares `DataKey`, but the decoded interface has unions {:?}",
                interface
                    .unions
                    .iter()
                    .map(|union| union.name.as_str())
                    .collect::<Vec<_>>()
            )
        });

    assert_eq!(union.cases.len(), 1, "`DataKey` declares one case");
    assert_eq!(union.cases[0].name, "Sequence");
    assert_eq!(union.cases[0].type_names, vec!["address".to_owned()]);
}

/// The interface's doc comments are carried into the module and decoded.
///
/// The SDK embeds the Rust doc comment of each entry point, so the engine's `doc` field
/// is checkable against a real artefact rather than only against a synthesised one. The
/// assertion looks for a phrase from the callee's `require_auth` paragraph, which no
/// other reading of the bytes could produce.
#[test]
fn the_callee_interface_carries_the_source_doc_comments() {
    let module = parse_module(&bytes(CALLEE_FILE)).expect("a real module parses");
    let interface = decode_spec_section(module.spec_section.as_deref().expect("a spec section"))
        .expect("decodes");

    let doc = interface
        .function("record")
        .and_then(|function| function.doc.as_deref())
        .expect("`record` documents itself in the source, so it is documented in the spec");

    assert!(
        doc.contains("require_auth"),
        "the decoded doc comment does not carry the source's words; it reads: {doc}"
    );
}

/// A real module's exports are its entry points, plus the two the toolchain adds.
///
/// The distinction between an export and a declared function is the suite's central
/// theme, and a real module is the case where both exist and differ. `memory` and `_`
/// are exported by the toolchain and are not interface functions.
#[test]
fn a_real_module_exports_its_entry_points_and_nothing_invented() {
    for (file, expected) in [
        (CALLEE_FILE, CALLEE_FUNCTIONS),
        (CALLER_FILE, CALLER_FUNCTIONS),
    ] {
        let module = parse_module(&bytes(file)).expect("a real module parses");
        let exported: Vec<&str> = module
            .exports
            .iter()
            .map(|export| export.name.as_str())
            .collect();

        for function in expected {
            assert!(
                exported.contains(&function.name),
                "{file}: `{}` is declared but not exported. Exports: {exported:?}",
                function.name
            );
        }
        for support in SDK_EXPORTS {
            assert!(
                exported.contains(support),
                "{file}: the toolchain normally exports `{support}`. Exports: {exported:?}"
            );
        }
        assert_eq!(
            exported.len(),
            expected.len() + SDK_EXPORTS.len(),
            "{file}: an unexpected export appeared. Exports: {exported:?}"
        );
    }
}

/// A real SDK 27 module does not import its host functions from a namespace named
/// `env`, so `host_function_imports` finds none of them.
///
/// # This assertion pins a limitation, not a correctness
///
/// `WasmModule::host_function_imports` returns only imports whose module is `env`, and
/// its documentation states that this is a protocol fact: that Soroban host functions
/// live in the `env` namespace. For modules built by the pinned SDK that is not true.
/// The eleven imports of the callee are its host functions - reading the module's own
/// `name` section in order gives `vec_new_from_linear_memory`,
/// `symbol_new_from_linear_memory`, `address::require_auth`, `context::contract_event`,
/// `obj_to_i128_hi64`, `obj_to_i128_lo64`, `ledger::get_contract_data`,
/// `ledger::has_contract_data`, `ledger::put_contract_data`, `obj_from_i128_pieces` and
/// `ledger::extend_contract_data_ttl`, which is exactly the set the callee's source
/// calls and exactly the order the import section lists them in - but the module and
/// field strings are generated one- and two-character names such as `v.g` and `l.0`.
///
/// The engine's analysis does not use the method, so nothing it reports is wrong today;
/// what is wrong is the documentation's claim about the protocol, and this test exists
/// so that the limitation is recorded against a real artefact rather than believed. The
/// function is asserted to be non-empty-guarded below, so the assertion cannot pass
/// vacuously by the module having no imports at all.
#[test]
fn host_function_imports_does_not_recognise_sdk_27_host_imports() {
    let module = parse_module(&bytes(CALLEE_FILE)).expect("a real module parses");

    assert!(
        !module.imports.is_empty(),
        "the callee imports its host functions, so this assertion must not pass vacuously"
    );
    assert!(
        module.host_function_imports().is_empty(),
        "the pinned SDK's host imports are no longer outside the `env` namespace, which \
         means either the SDK changed or this finding is stale. Imports: {:?}",
        module
            .imports
            .iter()
            .map(|import| format!("{}.{}", import.module, import.field))
            .collect::<Vec<_>>()
    );
}

/// Both modules carry the SDK's environment metadata.
#[test]
fn both_reference_modules_carry_environment_metadata() {
    for file in [CALLEE_FILE, CALLER_FILE] {
        let module = parse_module(&bytes(file)).expect("a real module parses");
        let payload = module
            .env_meta_section
            .as_deref()
            .unwrap_or_else(|| panic!("{file}: `{CONTRACT_ENV_META_SECTION}` is absent"));
        assert!(
            !payload.is_empty(),
            "{file}: the environment metadata section is present but empty"
        );
    }
}

/// Asserts a module's decoded interface equals `expected`, in every field.
fn assert_interface(file: &str, expected: &[ExpectedFunction]) {
    let module = parse_module(&bytes(file)).expect("a real module parses");
    let interface = decode_spec_section(module.spec_section.as_deref().expect("a spec section"))
        .expect("decodes");

    assert_eq!(
        interface.undecodable_trailing_bytes, 0,
        "{file}: the interface decoded with bytes left over, so the reading is partial \
         ({:?})",
        interface.decode_detail
    );
    assert_eq!(
        interface.functions.len(),
        expected.len(),
        "{file}: decoded {} functions, expected {}. Decoded: {:?}",
        interface.functions.len(),
        expected.len(),
        interface
            .functions
            .iter()
            .map(|function| function.name.as_str())
            .collect::<Vec<_>>()
    );

    for want in expected {
        let got = interface
            .function(want.name)
            .unwrap_or_else(|| panic!("{file}: `{}` was not decoded", want.name));

        let inputs: Vec<(&str, &str)> = got
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), input.type_name.as_str()))
            .collect();
        assert_eq!(
            inputs,
            want.inputs.to_vec(),
            "{file}: `{}` decoded with the wrong parameters",
            want.name
        );
        assert_eq!(
            got.output, want.output,
            "{file}: `{}` decoded with the wrong return type",
            want.name
        );
        assert!(
            got.doc.as_deref().is_some_and(|doc| !doc.is_empty()),
            "{file}: `{}` lost its doc comment, which the SDK writes into the section",
            want.name
        );
    }
}
