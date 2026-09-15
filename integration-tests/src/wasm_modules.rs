//! Real WebAssembly modules, deliberately without a contract interface.
//!
//! # Why they are minimal and why that is the point
//!
//! These are genuine, spec-conformant WebAssembly modules: the magic number, the
//! version, and real section encodings that any decoder accepts. They are not Soroban
//! contracts, and the absence is the assertion.
//!
//! The engine's rule is that it must distinguish what it observed from what it could
//! not obtain. A module with no `contractspecv0` section has no interface to report,
//! and the failure mode this corpus exists to catch is a tool that invents one - a
//! plausible-looking function list that no byte of the module supports. So the corpus
//! holds a module with an export that is *not* a contract function, a module with no
//! sections at all, and two byte sequences that are not modules at all, and the
//! `contracts` suite asserts that each is classified for what it is.
//!
//! # The encodings
//!
//! A module is `magic || version || sections*`, where a section is
//! `id (u8) || size (LEB128) || contents`. The three built here are:
//!
//! * an empty module: the eight header bytes and nothing else;
//! * a module with one custom section named `amasario-fixture`, which a decoder must
//!   skip rather than reject;
//! * a module with a type section, a function section and an export section that
//!   exports `greet`, so that a reader that looks only for exports would find one.

use amasario_contract::wasm::{WASM_MAGIC, WASM_VERSION};

/// The eight header bytes every module starts with.
#[must_use]
pub fn header() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&WASM_MAGIC);
    bytes.extend_from_slice(&WASM_VERSION);
    bytes
}

/// A module with a header and no sections.
///
/// Valid, and empty in the strongest sense: there is nothing to decode and therefore
/// nothing an interface could be derived from.
#[must_use]
pub fn empty_module() -> Vec<u8> {
    header()
}

/// A module whose only section is a custom one.
///
/// `custom` is section id 0, and its contents are a name followed by uninterpreted
/// bytes. The engine must skip it, so a decoder that failed on an unknown section name
/// would fail here.
#[must_use]
pub fn custom_section_module() -> Vec<u8> {
    const NAME: &[u8] = b"amasario-fixture";
    const PAYLOAD: &[u8] = b"test";

    let mut bytes = header();
    // Section id 0 is `custom`.
    bytes.push(0);
    // The section's size is the name's length prefix, the name, and the payload.
    let size = 1 + NAME.len() + PAYLOAD.len();
    bytes.push(u8::try_from(size).expect("a section smaller than 128 bytes"));
    bytes.push(u8::try_from(NAME.len()).expect("a name shorter than 128 bytes"));
    bytes.extend_from_slice(NAME);
    bytes.extend_from_slice(PAYLOAD);
    bytes
}

/// A module that exports `greet` as a function and has no contract spec section.
///
/// The export is real: the type section declares one function type with no parameters
/// and no results, the function section binds index 0 to it, and the export section
/// names index 0 `greet`. A reader that treated exports as the contract interface would
/// report `greet` - and that is precisely the inference the engine must refuse, because
/// a WebAssembly export appears in a Soroban contract's interface only when the
/// contract spec says so.
#[must_use]
pub fn module_with_export_only() -> Vec<u8> {
    let mut bytes = header();

    // Type section (id 1): one function type, `() -> ()`. The declared payload length
    // is 4, which is the count byte, the `0x60` form byte, and the two empty vectors.
    bytes.extend_from_slice(&[0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
    // Function section (id 3): one function, of type index 0.
    bytes.extend_from_slice(&[0x03, 0x02, 0x01, 0x00]);
    // Export section (id 7): one export, named `greet`, of kind function, index 0.
    // The payload is the count byte, the length-prefixed name, the kind byte and the
    // index byte: 1 + 1 + 5 + 1 + 1 = 9. A section whose declared length disagrees with
    // its contents is the defect a decoder is supposed to catch, so the arithmetic here
    // matters.
    bytes.extend_from_slice(&[0x07, 0x09, 0x01, 0x05]);
    bytes.extend_from_slice(b"greet");
    bytes.extend_from_slice(&[0x00, 0x00]);

    bytes
}

/// A header truncated to four bytes: the magic, with no version.
///
/// Not a module. A reader that checked only the magic number would accept it and then
/// read the version out of bounds.
#[must_use]
pub fn truncated_header() -> Vec<u8> {
    WASM_MAGIC.to_vec()
}

/// A module whose version word is wrong.
///
/// Not a module, and the failure a reader would otherwise miss: the header is the right
/// length and the magic is right, so only a version check catches it.
#[must_use]
pub fn wrong_version() -> Vec<u8> {
    let mut bytes = WASM_MAGIC.to_vec();
    bytes.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
    bytes
}

/// One corpus entry: a name, the bytes, and what they are.
///
/// `magic_present` is deliberately a different question from `is_a_module`. The engine's
/// `looks_like_module` checks the magic number alone, because its documented purpose is
/// to be a cheap pre-check before paying to parse; the corpus records that as its own
/// field rather than conflating it with decodability, which is what an earlier version
/// of this module got wrong.
#[derive(Debug, Clone, Copy)]
pub struct ModuleSpec {
    /// The fixture's file stem, without the extension.
    pub name: &'static str,
    /// The module's bytes.
    pub bytes: fn() -> Vec<u8>,
    /// Why this module is in the corpus.
    pub note: &'static str,
    /// Whether the four magic bytes are present.
    pub magic_present: bool,
    /// Whether the whole module decodes.
    pub is_a_module: bool,
}

/// Every module this corpus defines.
#[must_use]
pub fn all() -> Vec<ModuleSpec> {
    vec![
        ModuleSpec {
            name: "empty",
            bytes: empty_module,
            note: "a valid module with no sections at all, so nothing can be decoded from it",
            magic_present: true,
            is_a_module: true,
        },
        ModuleSpec {
            name: "custom-section",
            bytes: custom_section_module,
            note: "a valid module whose only section is a custom one, which a decoder must skip",
            magic_present: true,
            is_a_module: true,
        },
        ModuleSpec {
            name: "export-without-spec",
            bytes: module_with_export_only,
            note: "a valid module exporting `greet` and declaring no contract spec section, so \
                   no contract interface may be reported for it",
            magic_present: true,
            is_a_module: true,
        },
        ModuleSpec {
            name: "truncated-header",
            bytes: truncated_header,
            note: "the magic number with no version word: the pre-check accepts it and the \
                   decoder must not",
            magic_present: true,
            is_a_module: false,
        },
        ModuleSpec {
            name: "wrong-version",
            bytes: wrong_version,
            note: "the right magic and the wrong version word, which only a version check catches",
            magic_present: true,
            is_a_module: false,
        },
    ]
}

/// The lowercase hex of a module's bytes, which is how a fixture stores them.
///
/// Hex rather than base64 because the fixture is meant to be read: a maintainer
/// comparing a recorded digest against the bytes should be able to see where a module
/// begins and ends without a decoder.
#[must_use]
pub fn hex_of(bytes: &[u8]) -> String {
    hex::encode(bytes)
}
