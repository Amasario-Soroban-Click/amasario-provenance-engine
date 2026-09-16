# AMASARIO — Provenance Engine

[![CI](https://github.com/Amasario-Soroban-Click/amasario-provenance-engine/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Amasario-Soroban-Click/amasario-provenance-engine/actions/workflows/ci.yml)
[![Testnet](https://github.com/Amasario-Soroban-Click/amasario-provenance-engine/actions/workflows/testnet.yml/badge.svg?branch=main)](https://github.com/Amasario-Soroban-Click/amasario-provenance-engine/actions/workflows/testnet.yml)
[![Security](https://github.com/Amasario-Soroban-Click/amasario-provenance-engine/actions/workflows/security.yml/badge.svg?branch=main)](https://github.com/Amasario-Soroban-Click/amasario-provenance-engine/actions/workflows/security.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust: 1.98.1](https://img.shields.io/badge/rust-1.98.1-orange.svg)](rust-toolchain.toml)
[![Specification: 1.0.0](https://img.shields.io/badge/amasario--spec-1.0.0-informational.svg)](https://github.com/Amasario-Soroban-Click/amasario-provenance-spec)

The **Testnet** badge is the live test: it runs on a schedule against a real contract on
testnet and asserts the analysis, not just that the commands exited zero. A red badge
there means the engine, or the world the committed target was chosen from, has changed.

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

Every layer below is complete: it compiles, it is lint-clean, and it has tests. The
counts are the ones `cargo test --workspace --all-features` reports, not estimates.

| Crate | Responsibility | Status |
| --- | --- | --- |
| `amasario-core` | Execution context, observation model, relationship semantics, pipeline coordination | implemented, 149 tests (+9 specification-conformance) |
| `amasario-network` | Stellar RPC adapters, pagination, retries, error classification | implemented, 102 tests |
| `amasario-contract` | Contract inspection: identity, executable hash, interface, storage | implemented, 109 tests |
| `amasario-provenance` | Source → build → artifact → wasm → deployment verification | implemented, 113 tests |
| `amasario-dependency` | Dependency discovery, classification and resolution | implemented, 102 tests |
| `amasario-graph` | Typed graph construction, traversal, paths, cycle detection | implemented, 107 tests |
| `amasario-impact` | Direct, transitive and multi-hop impact analysis | implemented, 76 tests |
| `amasario-evidence` | Evidence collection, verification and confidence calculation | implemented, 83 tests |
| `amasario-snapshot` | Snapshot capture, normalization, storage and comparison | implemented, 46 tests |
| `amasario-report` | JSON, Markdown, DOT and JUnit report generation | implemented, 29 tests |
| `amasario-export` | JSON, YAML, GraphML and DOT export | implemented, 27 tests |
| `amasario-cli` | The `amasario` binary: inspect, discover, provenance, dependencies, graph, impact, snapshot, diff, verify, report, export | implemented, 31 tests |

`cargo test --workspace --all-features` passes **1088 tests**, of which 974 are the unit
and per-crate suites of the twelve crates above and 114 are the ten end-to-end suites
in `integration-tests` — `network`, `captures`, `contracts`, `provenance`, `dependencies`,
`graphs`, `impact`, `snapshots`, `verification` and `reports`.

Six of those 114 do not generate anything: `captures` serves the four responses under
`fixtures/` that were read off testnet verbatim, over a real socket, into the adapters'
own client — the suite that exists because three live defects were invisible in every
hand-written document. The other eight suites run against a generated corpus. None of the
ten reaches the live network, so CI does not go flaky because a public endpoint was busy;
the live test is `testnet.yml`, which is scheduled rather than triggered by a pull request.

The nine specification-conformance tests in `amasario-core` are `#[ignore]`d by default
because they read a checkout of the normative specification; CI provides one and runs
them with `--include-ignored`. Nine ignored tests are therefore expected in a plain
`cargo test` run, and none of them are skipped work.

Nothing in the table is a placeholder: a crate is listed as implemented only when it
compiles, is lint-clean and has tests. A crate that does not exist yet is not listed
as existing.

The workspace now builds a binary as well as libraries. `cargo run -p amasario-cli --`,
or the installed `amasario`, exposes eleven subcommands:

```console
amasario inspect      --contract CABC... --network testnet
amasario discover     --contract CABC... --network testnet
amasario provenance   --contract CABC... --network testnet --artifact-digest <hex>
amasario dependencies --contract CABC... --network testnet --depth 5
amasario graph        --contract CABC... --network testnet --format dot
amasario impact       --contract CABC... --network testnet --change-type MODIFIED
amasario snapshot create --contract CABC... --network testnet --output snapshots/
amasario snapshot show   --input snapshots/<file>.json
amasario diff         --before a.json --after b.json
amasario verify       --contract CABC... --network testnet --require verified
amasario report       --contract CABC... --network testnet --format markdown
amasario export       --input snapshot.json --format graphml
```

The result goes to stdout - or to the path named by `--output` - and narration goes to
stderr, so a pipeline may consume stdout without stripping commentary. A run exits zero
only when it completed and passed any gate it was asked to enforce; a usage problem
exits 2, a failed gate exits 1, and an engine failure exits with a code derived from the
specification's error category, so a CI job can tell a network failure from a provenance
contradiction without parsing a message.

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

## Use it in your own CI

The engine ships a composite action, so a project that wants a dependency or verification
gate does not have to work out how to build it first:

```yaml
- uses: Amasario-Soroban-Click/amasario-provenance-engine/.github/actions/amasario@v1
  with:
    contract: CABC...
    network: testnet
    command: dependencies
    args: --scan-events --depth 2
    output: dependency-report.md
```

The action builds the engine from the ref you pinned, so the action and the engine cannot
drift, and it writes the rendered document to the job summary as well as to a file. Both
the action and the engine are read-only: nothing in this repository submits a
transaction.

A release gate reads `verify` and branches on a status that carries its own limits —
`scope.checked` and `scope.notChecked` say what was compared and what was not, so a
pipeline does not have to take `VERIFIED` for more than it is:

```yaml
- uses: Amasario-Soroban-Click/amasario-provenance-engine/.github/actions/amasario@v1
  with:
    contract: CABC...
    command: verify
    require: partially
    format: json
    output: verification.json
```

Both examples run against a real network, so both need a reachable endpoint. For a gate
that must not depend on a public service, run the CLI against a local standalone chain, or
compare two committed snapshots with `amasario diff`, which reads files and nothing else.

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

## Documentation

| Page | What it covers |
| --- | --- |
| [`docs/architecture.md`](docs/architecture.md) | Crate boundaries, the pipeline, and why the split is where it is |
| [`docs/data-flow.md`](docs/data-flow.md) | One run end to end, from CLI arguments to an exported document |
| [`docs/cli.md`](docs/cli.md) | Every subcommand, its flags, its exit codes and its output |
| [`docs/contract-discovery.md`](docs/contract-discovery.md) | What inspection can and cannot obtain |
| [`docs/provenance.md`](docs/provenance.md) | The source → revision → build → artifact → wasm → deployment chain |
| [`docs/verification.md`](docs/verification.md) | The five verification statuses and how each is reached |
| [`docs/evidence.md`](docs/evidence.md) | Evidence records, confidence, and why confidence is not evidence |
| [`docs/dependency-analysis.md`](docs/dependency-analysis.md) | Discovery, classification and what is deliberately not inferred |
| [`docs/graph-analysis.md`](docs/graph-analysis.md) | Nodes, edges, traversal, paths and cycles |
| [`docs/impact-analysis.md`](docs/impact-analysis.md) | Direct, transitive and multi-hop impact |
| [`docs/snapshots.md`](docs/snapshots.md) | Capture, normalisation, the content digest and comparison |
| [`docs/output-formats.md`](docs/output-formats.md) | JSON, YAML, Markdown, DOT, GraphML and JUnit |
| [`docs/testnet.md`](docs/testnet.md) | Running against testnet, and the observation boundary |
| [`docs/ci-integration.md`](docs/ci-integration.md) | Using the engine as a build gate |
| [`docs/security.md`](docs/security.md) | The security boundaries, stated as what the tool does not claim |
| [`docs/fuzzing.md`](docs/fuzzing.md) | The five fuzz targets, what each asserts, and how a failure is read |
| [`docs/troubleshooting.md`](docs/troubleshooting.md) | Exit codes, and each failure mode with its cause and remedy |
| [`examples/README.md`](examples/README.md) | Nine worked examples, and which of them run without a network |

## Examples

Nine worked examples live under [`examples/`](examples/), one per question the engine is
built to answer. They are executable documentation rather than prose: each directory has
a `command` script, and `scripts/check-examples.sh` runs the offline ones and compares
their output against the committed bytes.

The distinction the tree draws is honest about what can be verified where. An **offline**
example reads the generated corpus under [`fixtures/`](fixtures/) and reproduces exactly,
so CI checks it. An **endpoint** example analyses something only a chain can describe, so
it cannot run in CI and does not pretend to - instead the check points it at an address
that refuses connections and requires exit `69` (`NETWORK`), which proves every flag in
the documented command line parses without needing a network to analyse.

```console
scripts/check-examples.sh
```

## Licence

Apache-2.0. See [`LICENSE`](LICENSE).
