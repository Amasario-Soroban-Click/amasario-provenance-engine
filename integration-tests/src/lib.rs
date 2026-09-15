//! The integration corpus: fixture builders, recorded endpoint responses, and the
//! harnesses the suites and the fuzz targets share.
//!
//! # Why this is a library rather than a `tests/` directory
//!
//! Nine suites need the same fixture builders. Cargo builds every integration test as
//! its own crate, so a builder duplicated across nine files is nine copies that can
//! disagree - and the corpus is exactly the artefact that must not disagree with
//! itself. A shared library target is the way to give them one definition.
//!
//! The suites themselves live in the nine directories beside this manifest
//! (`network/`, `contracts/`, ..., `reports/`), declared as explicit `[[test]]`
//! targets, so that a reader looking for the graph suite finds it under `graphs/`.
//!
//! # What belongs here
//!
//! * [`documents`] builds every fixture document from the engine's own types, so a
//!   committed fixture cannot say something the model would refuse to construct.
//! * [`corpus`] locates the `fixtures/` tree and compares committed bytes against
//!   freshly built ones, which is what stops a fixture drifting from its builder.
//! * [`recordings`] holds synthetic endpoint responses. They are shaped like the
//!   documented Stellar RPC and Horizon responses and are labelled as recordings
//!   rather than as observations: nothing here was read off a live network.
//! * [`captures`] holds the opposite: four responses read off testnet verbatim,
//!   with the request and the day recorded beside each. They exist because three
//!   live defects were invisible in every hand-written document, and each of the
//!   three is now asserted against the bytes that exposed it.
//! * [`wasm_modules`] holds real, minimal WebAssembly modules and their digests.
//!   They are deliberately *not* Soroban modules, because the interesting assertion
//!   is that a module with no contract-spec section yields an unknown interface
//!   rather than a fabricated one.
//! * [`harness`] holds the invariant checks the fuzz targets run, so that the same
//!   logic is exercised by an ordinary test rather than only under a nightly fuzzer.
//! * [`cli`] locates the `amasario` binary so the end-to-end suites can run it.
//! * [`corpus_readme`] generates the corpus's own index, so that a directory a reader
//!   opens without running anything is still a checked statement.

pub mod captures;
pub mod cli;
pub mod corpus;
pub mod corpus_readme;
pub mod documents;
pub mod harness;
pub mod recordings;
pub mod wasm_modules;

pub use corpus::Corpus;
