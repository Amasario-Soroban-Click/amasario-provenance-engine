//! The first-party reference contract: where it is, and what it is.
//!
//! # Why this exists
//!
//! Two things were true of the corpus before this module, and together they left a
//! gap in exactly the place that mattered most.
//!
//! The modules in `fixtures/wasm/` are hand-assembled byte sequences, and deliberately
//! so: their purpose is to assert what the engine must *refuse* to report, and the
//! module that really does export `greet` while declaring no interface is the fixture
//! that catches a tool which lists exports as a contract's functions. None of them is
//! a module the Soroban SDK produced, so nothing in the corpus exercised
//! `contractspecv0` decoding, `contractenvmetav0` reading or the section walk against
//! bytes the real toolchain emitted.
//!
//! The live suite made up some of the difference, but against a single contract on
//! testnet that does not belong to this project. A test whose failure has to be
//! triaged into "the engine changed" or "the world changed" is a weak oracle for the
//! project's flagship claim, and if that contract stops trading the claim loses its
//! verification quietly.
//!
//! So there is a real contract under `reference-contract/`, built from source in this
//! repository by `scripts/build-reference-contract.sh`, and committed here under
//! `fixtures/reference/`. Two contracts, in fact, with a deliberate call graph: the
//! `caller` invokes the `callee`, and `caller -> callee` is the edge the live analysis
//! is asserted to find.
//!
//! # What is checked, and against what
//!
//! The modules are committed as the `.wasm` files the build produced, which is a
//! departure from the rest of `fixtures/`. It is the right one here: hex inside a JSON
//! descriptor is how a *synthesised* module is recorded, whereas these are the
//! artefacts themselves - the thing a reviewer would deploy, and the thing a provenance
//! tool exists to talk about. Each has a JSON record beside it stating the toolchain
//! that built it and the digest of its bytes, and `each_module_matches_its_provenance`
//! recomputes the digest from the module rather than trusting the record.
//!
//! Those records are transcribed rather than generated, unlike every other fixture in
//! the corpus. That is precisely why the assertion exists: a record that cannot be false
//! is worth having, and a hand-written digest that is checked is no longer a claim.

use std::fs;

use amasario_contract::wasm::digest_of;
use serde::Deserialize;

use crate::corpus::Corpus;

/// The callee module's file name within `fixtures/reference/`.
pub const CALLEE_FILE: &str = "reference-callee.wasm";

/// The caller module's file name within `fixtures/reference/`.
pub const CALLER_FILE: &str = "reference-caller.wasm";

/// The callee module's digest, as recorded in `reference-callee.json` beside it.
///
/// Repeated here rather than read from that record so that a mismatch is a failing test
/// rather than a value that agrees with itself by construction. The pair is checked by
/// the `each_module_matches_its_provenance_record` test in `integration-tests/reference/`,
/// which is named in prose rather than linked because it lives in a separate target that
/// rustdoc for this crate does not see - and an intra-doc link to a test that is not in
/// scope is a broken link rather than a pointer.
pub const CALLEE_DIGEST: &str = "347286105091f6d2d5db47ef5ae4442550a4d18f3a0f11284b59c22211031ac7";

/// The caller module's digest, as recorded in `reference-caller.json` beside it.
pub const CALLER_DIGEST: &str = "5b9002d177b278725322f3b9e9ab95322e35e802203579d946d00e9627ca9ef0";

/// The directory the reference modules are committed in.
pub const DIRECTORY: &str = "reference";

/// One entry point the contract declares, as it must appear in the decoded interface.
#[derive(Debug, Clone, Copy)]
pub struct ExpectedFunction {
    /// The declared name.
    pub name: &'static str,
    /// The parameters, as `(name, declared type)`, in the order the decoded
    /// interface reports them.
    pub inputs: &'static [(&'static str, &'static str)],
    /// The declared return type, as the interface's own vocabulary spells it.
    pub output: &'static str,
}

/// The callee's declared interface, exactly.
///
/// `record` takes an `Address` and an `i128` and returns a `u32`; `sequence` takes an
/// `Address` and returns a `u32`. The input order is the *decoded* order, which is
/// sorted by name rather than the order the Rust signature declares them in -
/// `decode_spec_section` canonicalises, and a test written from the source signature
/// would encode that mistake.
pub const CALLEE_FUNCTIONS: &[ExpectedFunction] = &[
    ExpectedFunction {
        name: "record",
        inputs: &[("account", "address"), ("amount", "i128")],
        output: "u32",
    },
    ExpectedFunction {
        name: "sequence",
        inputs: &[("account", "address")],
        output: "u32",
    },
];

/// The caller's declared interface, exactly.
pub const CALLER_FUNCTIONS: &[ExpectedFunction] = &[
    ExpectedFunction {
        name: "observe",
        inputs: &[("account", "address"), ("ledger", "address")],
        output: "u32",
    },
    ExpectedFunction {
        name: "record_via",
        inputs: &[
            ("account", "address"),
            ("amount", "i128"),
            ("ledger", "address"),
        ],
        output: "u32",
    },
];

/// The functions each module exports to the host, above the ones the SDK adds.
///
/// `memory` and `_` are added by the toolchain and are not part of either contract's
/// declared interface, which is the distinction the pair of assertions is there to
/// keep visible.
pub const SDK_EXPORTS: &[&str] = &["memory", "_"];

/// The custom sections a real `soroban-sdk` 27 module carries.
///
/// Recorded because the engine's section walk is what reads them, and because the set
/// is a property of the toolchain rather than of these two contracts: if the SDK adds
/// or drops one, the engine's readers need to know.
pub const SDK_CUSTOM_SECTIONS: &[&str] = &[
    "contractspecv0",
    "contractenvmetav0",
    "contractmetav0",
    "name",
    "producers",
    "target_features",
];

/// The path of one reference module.
#[must_use]
pub fn path(file: &str) -> std::path::PathBuf {
    Corpus::path(DIRECTORY, file)
}

/// The bytes of one reference module, as committed.
///
/// # Panics
///
/// Panics when the module is absent, naming the script that produces it. The modules
/// are build output that is committed deliberately, so an absent one means the
/// checkout is incomplete rather than that the engine found nothing.
#[must_use]
pub fn bytes(file: &str) -> Vec<u8> {
    let path = path(file);
    fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "{} could not be read ({error}). The reference contract is built by \
             `scripts/build-reference-contract.sh`; its modules are committed under \
             `fixtures/reference/`",
            path.display()
        )
    })
}

/// The digest of one committed reference module, recomputed from its bytes.
#[must_use]
pub fn digest_value(file: &str) -> String {
    digest_of(&bytes(file)).value().to_owned()
}

/// The toolchain that produced a reference module.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Toolchain {
    /// The `soroban-sdk` version, which is pinned exactly in the contract's manifest.
    pub soroban_sdk: String,
    /// The compiler version, which is pinned by `rust-toolchain.toml`.
    pub rustc: String,
    /// The target the module was built for.
    pub target: String,
}

/// One module's provenance record, as committed beside it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Provenance {
    /// The module's name.
    pub name: String,
    /// Why this module is in the corpus.
    pub note: String,
    /// The directory under `reference-contract/` it is built from.
    pub builds_from: String,
    /// The toolchain that produced it.
    pub toolchain: Toolchain,
    /// The module's file name within this directory.
    pub module_file: String,
    /// The module's size in bytes.
    pub byte_size: usize,
    /// The SHA-256 digest of the module's bytes.
    pub digest: String,
    /// The algorithm the digest is computed with.
    pub digest_algorithm: String,
}

/// The provenance record for one module, read from the JSON beside it.
///
/// # Panics
///
/// Panics when the record is absent or does not parse, because a module without its
/// record is an artefact whose origin is unstated, which is the thing this directory
/// exists to avoid.
#[must_use]
pub fn provenance(module_file: &str) -> Provenance {
    let stem = module_file.trim_end_matches(".wasm");
    let path = path(&format!("{stem}.json"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read ({error})", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a provenance record ({error})", path.display()))
}
