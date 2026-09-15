# Changelog

All notable changes to `amasario-provenance-engine` are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Two
version numbers move independently and are both meaningful:

- **This engine's version** (below) - the implementation.
- **The specification version it supports** (`SUPPORTED_SPEC_VERSION` in
  `crates/amasario-core/src/engine.rs`). The engine refuses a document from a newer minor
  version rather than guessing at a field it does not implement, and refuses a different
  major family outright.

A change that alters what a document means is a breaking change for a consumer even when
no function signature changed, so the entries below call those out explicitly.

## [1.0.0] - unreleased

No version of this engine has been published yet. Everything below is present on the
default branch and passes the checks in `scripts/run-ci.sh`. It is listed as `1.0.0`
because that is the version in the workspace manifest and the specification version it
implements, not because it has been released.

### Added - the specification layer it implements

The engine implements specification `1.0.0` (`apiVersion` `amasario.dev/v1`) and consumes
it rather than redefining it.

- **Document projections.** The engine's internal model is richer than the published
  documents, and each published document is a projection of it rather than the internal
  type with a different name. `GraphDocument`/`EdgeDocument`, `DependencyDocument`/
  `DependencySetDocument`, `EvidenceDocument`, and the report, snapshot and diff documents
  are all projections, so an internal field can never become part of a published shape by
  accident.
- **Wire-format conformance.** Every published document uses the specification's
  camelCase field names, because the schemas set `additionalProperties: false`: a document
  is either exactly the schema's shape or it is rejected. `GraphDocument` validates against
  `graph.schema.json` with zero errors, as do the dependency, dependency-set, evidence and
  report documents.
- **Specification conformance tests.** `crates/amasario-core/tests/` reads a checkout of
  the normative specification and fails when the engine's error categories, relationships,
  evidence types or statuses drift from the taxonomies. `#[ignore]`d by default because
  they need that checkout; CI provides one and runs them with `--include-ignored`.
- **Profile validation.** `scripts/validate-profile.sh` checks the Stellar/Soroban profile
  against the specification's taxonomies, including that each network id is the SHA-256 of
  the passphrase recorded beside it.

### Added - the analysis layers

- **`amasario-core`** - execution context, the observation model, relationship semantics
  with change propagation, the error model with the specification's categories, and the
  deterministic pipeline. The error model is total: every failure names exactly one
  category, and `ErrorCategory`'s wire names are asserted against `error.schema.json`.
- **`amasario-network`** - Stellar RPC and Horizon adapters over the Stellar
  organisation's own client crates. Bounded retries, pagination, timeouts, and an error
  classification in which a network failure is never an empty result. The network
  identity is checked before anything is observed, so an analysis cannot accumulate facts
  from one chain and be labelled with another.
- **`amasario-contract`** - contract inspection: identity separated from observation, the
  deployed module retrieved and its bytes hashed against the digest the network records,
  interface decoding, instance storage, and observed invocations. A contradiction is an
  anomaly on an otherwise complete inspection rather than an error that aborts the run.
- **`amasario-provenance`** - the source → build → artifact → wasm → deployment chain, with
  matching, verification and attestation handling. A claimed revision that cannot be
  matched to the deployed module is `CONFLICTING`, never `VERIFIED`, and a `CONFLICTING`
  outcome takes precedence over every other status.
- **`amasario-dependency`** - dependency discovery from observed invocations, classification
  against the taxonomy's evidence requirements, and resolution. An observation that cannot
  establish a dependency is set aside with a reason rather than dropped or upgraded.
- **`amasario-graph`** - the typed graph, with traversal, path discovery, depth bounds,
  cycle detection and deterministic serialisation. An edge carries its evidence, its
  confidence and whether it was observed or inferred.
- **`amasario-impact`** - direct and transitive propagation. A finding carries its path,
  hop depth, evidence, confidence, reason and verification state, and a bounded traversal
  is distinguishable from an exhausted one.
- **`amasario-evidence`** - evidence collection for the nine classes, the registry, the
  confidence calculation and the verification outcome. A confidence level always cites its
  evidence; a bare level is not constructible.
- **`amasario-snapshot`** - capture, normalisation, storage and canonical comparison. A
  snapshot's content digest covers everything except the specification's declared volatile
  fields, so two runs over the same facts produce the same bytes and a changed digest means
  a change in what was observed.
- **`amasario-report`** - the report document in JSON, Markdown, DOT and JUnit. The
  specification's requirement that observed facts, inferences, verification, confidence,
  unknowns and errors never merge is enforced in the model and in every renderer, and the
  required disclaimers cannot be removed.
- **`amasario-export`** - JSON, YAML, GraphML and DOT export of a graph document, with the
  lossless formats distinguished from the lossy ones rather than all four being called
  equivalent.

### Added - the command-line interface

- **`amasario-cli`** and the `amasario` binary, with eleven commands: `inspect`,
  `discover`, `provenance`, `dependencies`, `graph`, `impact`, `snapshot create`,
  `snapshot show`, `diff`, `verify`, `report` and `export`.
- **Classified exit codes.** A usage problem exits 2, a failed gate exits 1, and an engine
  failure exits with a code derived from the specification's error category, so a CI job
  can tell a network failure from a provenance contradiction without parsing a message.
- **stdout is the result and stderr is the narration**, so a pipeline may consume stdout
  without stripping commentary.
- **Read-only.** No command submits a transaction, and none requires a private key or a
  secret. Endpoint URLs embedding credentials are rejected before they can reach an error,
  a report or a snapshot.

### Added - repository infrastructure

- **`profiles/stellar-soroban.yaml`** - the Stellar/Soroban profile: the networks and what
  identifies them, the entity kinds observable from a chain and those that are not, the
  relationships assertable with the bases that establish them, and the guarantees and
  non-guarantees of a result from this profile.
- **`scripts/`** - `install.sh`, `install.ps1`, `run-ci.sh`, `test-testnet.sh`,
  `benchmark.sh`, `release.sh` and `validate-profile.sh`.
- **CI** - formatting, clippy with warnings denied, tests on three platforms, specification
  conformance, profile validation, rustdoc with broken links denied, and release
  validation.

### Security

- The engine states plainly, in the README, in the report's own required disclaimers and
  in `SECURITY.md`, that it is not a security scanner and that no output is a security
  assessment.
- No error variant has a field a credential could be stored in, so a secret cannot reach a
  log, a report or a snapshot through the error path.
- Build environments are sanitised: a value whose variable name looks like a secret is
  replaced with a redaction marker rather than recorded.

### Known limitations

- The engine does not read private source code. A claimed source revision is reported as
  unverified rather than fetched and checked.
- A contract deployed before the endpoint's retention window has no observable deployment
  operation. The engine reports the absence rather than a guess.
- Event feeds are pruned, so a cross-contract call that left no unpruned trace cannot be
  observed, and the absence of an observed call is not evidence that none occurred.
- The Stellar/Soroban profile is validated data and is not yet read by the CLI at run time;
  the CLI resolves networks through `amasario-network`. Wiring the profile into the command
  surface is separate work.
