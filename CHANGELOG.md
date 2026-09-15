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
- **An explicit event horizon.** `--lookback <LEDGERS>` covers the most recent stretch of
  history, `--from-ledger <LEDGER>` scans forward from a named ledger for a deliberate
  historical analysis, and `--max-event-pages <N>` bounds the requests. The horizon is the
  choice that decides whether a dependency answer describes the present or a week ago:
  `getEvents` paginates by event count rather than by ledger, so a scan that starts at what
  the node retains spends its whole page budget on the oldest few minutes of the window and
  never reaches recent activity. The default lookback is one day.

### Added - live-network verification

The engine's claims are checked against a chain, not only against fixtures.

- **A capture corpus.** Four responses read off testnet verbatim - `getHealth`, a contract
  `getEvents` page with a cursor, and two `getTransaction` calls, one that made a nested
  call and one that did not - are committed under `fixtures/` with the request that
  produced each and the day it was read. They are transcriptions rather than builder
  output, and the corpus README marks them as such. `integration-tests/captures` serves
  them over a real socket into the adapters' own client, so what is asserted is that the
  engine reads what the endpoint sent: an event page is read and a full page is reported as
  bounded rather than as the end, a start below the endpoint's floor is clamped and the
  request follows the clamp, the diagnostics are read from the transaction metadata, the
  nested capture yields a cross-contract edge naming its caller and inheriting the
  transaction's outcome, and the direct capture yields the one top-level call and no
  manufactured edge.
- **A live smoke test that asserts.** `scripts/test-testnet.sh` analyses a contract on a
  real network and checks the documents for what must hold for any successful analysis -
  identity resolved with a verified digest, every edge carrying evidence, a basis, a
  confidence level and an observation boundary, every status drawn from the specification's
  taxonomy, and the snapshot holding the edges the dependency analysis found - plus the one
  assertion the committed target exists for: a contract that calls another contract yields
  at least one verified `INVOCATES` edge. The target is committed in
  `scripts/live-target.env` and read by both the script and the workflow.
- **A composite action.** `.github/actions/amasario` builds the engine from the ref a
  caller pins and runs one read-only analysis, so another project can adopt the engine
  without working out how to build it. Its command allow-list is checked against the CLI's
  own subcommand list by a test, and the tests assert every workflow parses as YAML.
- **Badges.** CI, the scheduled live test, the security checks, the licence, the pinned
  toolchain and the specification version.

### Added - repository infrastructure

- **`profiles/stellar-soroban.yaml`** - the Stellar/Soroban profile: the networks and what
  identifies them, the entity kinds observable from a chain and those that are not, the
  relationships assertable with the bases that establish them, and the guarantees and
  non-guarantees of a result from this profile.
- **`scripts/`** - `install.sh`, `install.ps1`, `run-ci.sh`, `test-testnet.sh`,
  `benchmark.sh`, `release.sh` and `validate-profile.sh`.
- **CI** - formatting, clippy with warnings denied, tests on three platforms, specification
  conformance, profile validation, rustdoc with broken links denied, shell scripts linted
  with `shellcheck` at style severity, workflows and the composite action checked with
  `actionlint`, and release validation.
- **Dependency and workflow tooling pinned by digest.** `actionlint` is fetched at a pinned
  version and verified against a pinned SHA-256, because a supply-chain check should not be
  the weakest link in the supply chain.

### Changed

- **`amasario verify` states the limits of its own status.** `VERIFIED` for the executable
  identity is a comparison between two readings of one source: the module bytes the
  endpoint served and the executable digest the same endpoint reports. That is a real check
  - an endpoint contradicting itself is what `CONFLICTING` is for - but it is not
  independent corroboration, and the reason string now says so. The JSON document carries a
  `scope` member listing the comparison that was made and the questions the command does
  not ask: that the digest corresponds to any source revision or build, that any party
  other than the endpoint corroborates it, and anything about safety. A pipeline that
  branches on a bare `"status": "VERIFIED"` without reading `scope` is reading a narrower
  claim than the word suggests.
- **`snapshot create` in the smoke test passes `--scan-events`.** A snapshot is what every
  later step reads, so one captured without looking for dependencies would make the diff,
  the exports and the impact analysis all rest on a snapshot that recorded nothing.

### Fixed

Each of these was found by running the engine against testnet, and each was invisible in
fixtures that were shaped by hand.

- **The event horizon was the oldest edge of the retention window.** A scan started at the
  ledger the node reported as its oldest, so on a busy contract it read the oldest few
  minutes of a seven-day window, reported truncation, and found no recent activity. It now
  scans from the recent end.
- **A start ledger below the event index's own floor was rejected rather than clamped.**
  `getHealth` reports a lower floor than the index serves, so a scan measured against the
  health value failed at the edge of the window instead of observing it. The clamp now
  carries a margin and the captured responses show why it is needed: the two floors in the
  corpus are twelve ledgers apart.
- **A nested call was attributed to its caller.** A `fn_call` diagnostic carries the calling
  contract in its own `contract_id` field and the callee in its second topic. A decoder that
  read the field first attributed every nested call to the contract that made it, producing
  a graph in which every contract called nothing but itself - which is worse than an empty
  graph, because it looks like an answer.
- **The host's diagnostic events were read from the client's accessor, which the node
  leaves empty.** On the current protocol the node publishes diagnostics at the top level of
  the transaction response and inside the transaction metadata, and the `diagnosticEventsXdr`
  nested inside the response's `events` object - the one the bundled client reads - is
  empty. Trusting that accessor meant recovering no call nesting from any transaction and
  reporting every contract as having no dependencies.
- **A recovered call did not carry the transaction's outcome.** The dependency rules refuse
  to establish runtime use from a transaction whose outcome is unknown, so an invocation
  that inherited no outcome established nothing: the edges were extracted and then refused,
  and a live contract reported an empty dependency set with every call it made sitting in
  the evidence.
- **`nesting_recovered` was true for a transaction that did not nest.** It was computed as
  "the diagnostics yielded any invocations", so a contract entered once with no further
  calls - a root marker with no caller - reported reconstructed nesting. It now means at
  least one recovered call arrived from an enclosing one, so "this transaction nested" and
  "this transaction did not" are distinguishable, which is the distinction a control case
  exists to separate.
- **The scheduled live test could not fail, and the release job read an empty version.**
  The live job needed a repository variable that was never set, so it printed a notice and
  exited successfully without analysing anything, and the script asserted nothing. The
  release job cut its notes from `needs.validate.outputs.version` while declaring only
  `needs: publish`, so the version interpolated as the empty string.

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
- An event scan is bounded by a page budget as well as by a ledger window, because
  `getEvents` paginates by event count. A contract busy enough to fill every page the budget
  allows is reported as bounded rather than as fully covered, and `MAX_NODES_REACHED` is the
  truncation that says so. Raising the budget is a decision with a cost, and leaving it is
  one with a caveat.
- The scheduled live test depends on the committed target continuing to call another
  contract. If that contract goes quiet the test fails and says which of the two possible
  causes to check, and the remedy is a wider `--lookback` or a busier target rather than a
  change to the engine.
- The four captured responses are a recording of one endpoint on one day. Testnet is
  periodically reset, so a captured transaction hash will not resolve forever; the captures
  are committed precisely so the tests do not depend on it doing so.
