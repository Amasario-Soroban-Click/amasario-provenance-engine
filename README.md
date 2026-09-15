# AMASARIO — Provenance Engine

**Soroban contract dependency, provenance and impact infrastructure.**

This repository is `amasario-provenance-engine`, the **execution and analysis layer**
of Amasario. It consumes the normative models, schemas, rules and vectors defined by
[`amasario-provenance-spec`](https://github.com/Amasario-Soroban-Click/amasario-provenance-spec)
and performs contract inspection, network observation, evidence collection,
provenance verification, dependency discovery, graph construction, impact analysis,
snapshot creation, snapshot comparison and report generation.

The specification defines what the data and relationships **mean**. This repository
executes that specification. It does not redefine the normative model; where an
implementation type is required to consume a model, it lives here and says so.

## The question this engine answers

> What does this Soroban contract depend on, where did its deployed artifact come
> from, what evidence supports those relationships, and what other contracts or
> artifacts could be affected by a change?

## Status

The workspace is being built in verifiable batches, each of which compiles, is
tested, and is pushed only when it is complete. The current state is:

| Crate | Responsibility | Status |
| --- | --- | --- |
| `amasario-core` | Execution context, observation model, relationship semantics, pipeline coordination | implemented, 148 tests (+9 specification-conformance) |
| `amasario-network` | Stellar RPC adapters, pagination, retries, error classification | implemented, 98 tests |
| `amasario-contract` | Contract inspection: identity, executable hash, interface, storage | implemented, 103 tests |
| `amasario-provenance` | Source → build → artifact → wasm → deployment verification | implemented, 111 tests |
| `amasario-dependency` | Dependency discovery, classification and resolution | implemented, 85 tests |
| `amasario-graph` | Typed graph construction, traversal, paths, cycle detection | implemented, 106 tests |
| `amasario-impact` | Direct, transitive and multi-hop impact analysis | implemented, 75 tests |
| `amasario-evidence` | Evidence collection, verification and confidence calculation | implemented, 74 tests |
| `amasario-snapshot` | Snapshot capture, normalization, storage and comparison | implemented, 46 tests |
| `amasario-report` | JSON, Markdown, DOT and JUnit report generation | implemented, 29 tests |
| `amasario-export` | JSON, YAML, GraphML and DOT export | implemented, 27 tests |
| `amasario-cli` | The `amasario` command-line interface | not implemented |

`cargo test --all-features` passes 930 tests across the eleven crates above, including
the per-crate integration suites (`amasario-core`, `amasario-graph` and
`amasario-impact`). The nine specification-conformance tests in `amasario-core` are
`#[ignore]`d by default because they read a checkout of the normative specification;
CI provides one and runs them with `--include-ignored`.

Nothing in the table is a placeholder: a crate is listed as implemented only when it
compiles, is lint-clean and has tests. A crate that does not exist yet is not listed
as existing.

`amasario-cli` is the last crate outstanding. The workspace therefore builds libraries
only: there is no `amasario` binary and no CLI surface until it lands. Until then the
crates are consumed as a Rust library set, and every documented command in this README's
sibling projects is aspirational rather than runnable.

## Document conformance

The specification's schemas set `additionalProperties: false` and name their fields in
camelCase, so an engine document is either exactly the schema's shape or it is rejected.
The projections in `amasario-dependency`, `amasario-evidence` and `amasario-graph` are
validated against the published schemas themselves, and `GraphDocument`,
`DependencyDocument`, `DependencySetDocument`, `EvidenceDocument` and `Report` are all
checked to validate with zero errors.

A projection exists wherever the engine's internal model is richer than the document it
publishes - `GraphDocument` over `Graph`, `DependencyDocument` over `Dependency`,
`EvidenceDocument` over `EvidenceRecord`, `Report` over the analysis it describes. The
detail with no schema field is either placed in the schema's one open object, `metadata`,
or dropped with the loss written down in the module that drops it. It is never renamed
into a field that means something else.

## The four invariants `amasario-core` enforces

These are the properties the rest of the workspace is built on, and each is enforced
at the type level rather than documented and hoped for.

**An address is not an identity.** `ContractId` validates the shape of a Soroban
strkey; `EntityKind` distinguishes the address from the executable it hosts and from
the transaction that deployed it. Nothing in the crate can represent "the contract at
`C...`" as a complete identity, because an address survives an upgrade unchanged.

**A claim without evidence is not a claim.** `Confidence::new` refuses an empty
evidence list, so there is no way to construct a confidence level that names nothing.
`VERIFIED` therefore describes evidence completeness, and never the trustworthiness
of a contract.

**A contradiction must be representable.** `VerificationStatus` has a `Conflicting`
variant that takes precedence over every other status. When a claimed source revision
rebuilds to a different digest from the deployed module, the engine's only possible
answers are `CONFLICTING` or a bug - not `VERIFIED`. An incorrect `VERIFIED` is the
most damaging output this system can produce, because it is the one that stops a
reader looking further.

**A bounded search must say it was bounded.** `TruncationReason` distinguishes "the
search stopped" from "there was nothing left", including `RateLimited` as its own
value because that is the most common real cause of an incomplete traversal and the
one most easily mistaken for a complete result.

## What this engine does not claim

Amasario is provenance, dependency and impact infrastructure. It is **not** a security
scanner and no output is a security opinion. No term in the model means "secure",
"safe", "malicious" or "vulnerable", and the following must never be emitted or
implied:

`secure` · `safe` · `malicious` · `vulnerable` · `vulnerability-free` · `audited` · `certified`

`VERIFIED` means *the stated evidence is consistent with the stated claim*. Where a
third party genuinely performs an assessment, it is representable as an `ATTESTATION`
evidence record with an issuer, a method and explicit scope limitations - never as a
conclusion the engine reaches on its own. The engine reports factual states instead:
`OBSERVED`, `INFERRED`, `VERIFIED`, `PARTIALLY_VERIFIED`, `UNVERIFIED`, `CONFLICTING`,
`UNKNOWN`.

The engine also never logs secrets and never accepts them as command-line arguments,
because those appear in process history.

## Building

```bash
# The toolchain is pinned in rust-toolchain.toml, so this installs it if needed.
rustup show

cargo build --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo doc --no-deps --document-private-items
```

Or the aliases, which exist so nobody has to remember the flag lists:

```bash
cargo lint          # clippy over every target with warnings denied
cargo format-check  # rustfmt in check mode
cargo test-all      # the test suite with all features
```

## Determinism

Analysis results are deterministic for the same input, specification version,
observation boundary, available evidence and engine version. That requirement shapes
the code rather than being asserted about it:

- `ExecutionContext` takes the run's start time as an argument instead of reading the
  clock, so a test can pin it and two runs on the same input agree. The timestamp is
  excluded from every digest.
- Anything that reaches output is held in an ordered collection. `clippy.toml`
  refuses `HashMap` and `HashSet` by name, so this is checked rather than reviewed.
- Every bound is validated before a run starts, so an unusable configuration fails
  with a named problem rather than producing a shorter result that looks complete.

## Dependency choices

Significant dependencies, and why they are here rather than written by hand:

| Dependency | Why |
| --- | --- |
| `stellar-rpc-client` | Stellar's own RPC client, so the engine speaks the protocol the network implements instead of a locally invented approximation. |
| `stellar-xdr` | XDR decoding is a protocol fact; reimplementing it would be a correctness risk with no upside. |
| `stellar-strkey` | Dev-dependency only. The engine decodes strkeys by hand to avoid a runtime dependency, and the tests cross-check that decoder against Stellar's own encoder rather than against itself. |
| `serde_norway` | `serde_yaml` is unmaintained and `serde_yml` has not attracted the maintenance its predecessor lost. This is the maintained continuation of the original codebase. |
| `reqwest` with `rustls-tls` | Avoids an OpenSSL system dependency, which keeps a static build and cross-compilation realistic. |
| `petgraph` | Cycle detection and traversal over a typed graph, where a hand-written implementation would be the part of the code most likely to be subtly wrong. |
| `thiserror` | A structured error model is a requirement of the specification, not a convenience. |

`deny.toml` enforces the licence and source policy, and `cargo deny check` is part of
the security workflow.

## Relationship to the specification

The engine consumes the schemas, taxonomies, rules and vectors published by
`amasario-provenance-spec`. A change to the specification that would break a consumer
is detectable as a schema validation failure rather than as a silently different
analysis result, and the `vectors/` suite is the mechanism: the engine is expected to
reproduce each vector's canonical serialisation and digest byte for byte.

## Licence

Apache-2.0. See [`LICENSE`](LICENSE).
