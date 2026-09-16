# Architecture

The organisation is four repositories. This page describes the one boundary this
repository owns — the crate stack inside it, and what each crate may reach — and says only
enough about the other three to place it. The seams *between* repositories are in
[`amasario-docs`](https://github.com/Amasario-Soroban-Click/amasario-docs/blob/main/docs/architecture.md),
because no single repository can be the authority on them.

| Layer | Repository | Responsibility |
| --- | --- | --- |
| Normative | `amasario-provenance-spec` | Defines what contract identity, artifact identity, provenance, dependencies, evidence, confidence, snapshots and impact *mean*. Normative, versioned, machine-readable. |
| Execution | `amasario-provenance-engine` | Executes that specification against real networks: inspects contracts, collects evidence, resolves dependencies, builds graphs, verifies provenance, propagates impact, captures snapshots, compares them and writes reports. This repository. |
| Presentation | `amasario-explorer` | Renders documents this repository produced, and analyses nothing itself. |
| Cross-cutting | `amasario-docs` | The seams between the layers: policy, compatibility and the known gaps. |

The rows above are a reading order, not a dependency chain: the explorer depends on
committed documents rather than on this repository's code, which is why it can be written
in another language and why a change here cannot silently change a rendered picture.

This repository is the execution layer. It consumes the specification's schemas, taxonomies
and rules; it does not redefine them. Where an implementation type is needed to carry a
normative concept, that type is named after the concept and validated against the
specification's own vocabulary rather than a locally invented one.

## The rule that shapes everything

> The engine must never convert a failure into an empty result.

Every layer is built around that sentence. A network error is a classified error, not an
empty page. A dependency that was identified and refused by a rule is published as a
refusal, not dropped. An analysis that stopped at its bound says so, and one that could
not answer a question records the question. A reader of any engine document must be able
to tell "there is nothing here" from "this could not be determined".

## Layers

The crates form a stack. Each depends only on the ones below it, which is what keeps the
normative model from being reimplemented at every level.

```
        amasario-cli            the binary: eleven commands
             |
   +---------+---------+-----------------+
   |         |         |                 |
report    export    snapshot            |
   |         |         |                 |
   +---------+---------+-----------------+
             |
          impact
             |
           graph
             |
        dependency
             |
        provenance
             |
         evidence
             |
         contract
             |
         network
             |
           core
```

| Crate | What it owns |
| --- | --- |
| `amasario-core` | Configuration, observation boundaries, the relationship and basis vocabularies, identity primitives, error classification, the pipeline. |
| `amasario-network` | The RPC and Horizon adapters: paging, retries, timeouts, status classification, and the refusal to treat an absence as an empty result. |
| `amasario-contract` | Contract inspection and WebAssembly decoding: identity, module digest, interface observations, storage and invocation observations. |
| `amasario-evidence` | Evidence records, the collector, the verifier and confidence. |
| `amasario-provenance` | The source → revision → build → artifact → WASM → deployment chain, matching, verification outcomes and attestations. |
| `amasario-dependency` | Detection, classification, resolution and the bounded transitive closure. |
| `amasario-graph` | The typed graph: nodes, edges with derived identifiers, traversal, cycles, path search, deterministic serialisation. |
| `amasario-impact` | Bounded propagation and the findings it produces. |
| `amasario-snapshot` | Capture, canonicalisation, content digests, storage and comparison. |
| `amasario-report` | The specification's report document and its four renderings. |
| `amasario-export` | JSON, YAML, GraphML and DOT exports of a recorded analysis. |
| `amasario-cli` | The `amasario` binary: argument parsing, configuration resolution, output, exit codes. |

`amasario-integration-tests` is not part of the library stack: it is the workspace member
that owns the fixture corpus and the nine end-to-end suites.

## Determinism

Analysis is deterministic for the same input, specification version, network observation
boundary, available evidence and engine version. Three rules enforce it.

1. **Ordering is canonical, never incidental.** Anything that reaches output is sorted by
   an explicit key. `BTreeMap` and `BTreeSet` are used instead of the hash-ordered
   equivalents, and `clippy.toml` makes `HashMap` and `HashSet` a build error.
2. **Identifiers are derived, never assigned.** An edge's identifier is a digest over its
   endpoints and its relationship; a node's is its kind and its identifier; a change's is
   its category, its entity and its path. Two runs over the same evidence agree, and a
   diff of two runs can be a diff.
3. **A snapshot carries a content digest** over its canonical form with the declared
   volatile fields removed, so a snapshot can be checked against what it claims to be.

## Bounded work

Nothing in the engine recurses without a bound it reports.

* Traversal takes a depth bound and a node bound. `MAX_PERMITTED_DEPTH` is 32; a
  configured bound above it is clamped rather than refused, and the bound actually used is
  the one recorded in the result.
* A traversal that stopped early sets `truncated` **and** a `truncationReason`. A result
  that carries one without the other is refused.
* Path search has a separate cap on how many routes are reported.

The reason the disclosure matters is that its absence is indistinguishable from a
complete answer. A contract with no further dependencies and a contract whose graph was
not walked far enough both look like a short list.

## Error classification

`amasario-core` classifies every failure, and the classification decides whether the
caller retries:

| Category | Meaning | Retried |
| --- | --- | --- |
| Configuration | The request is wrong: a bad endpoint, a zero bound, a missing passphrase. | No |
| Network (transient) | A timeout, a connection failure, `429`, `408`, `5xx`. | Yes, with backoff |
| Network (permanent) | A rejected request, a `4xx` other than the retryable set. | No |
| Absence | `404`: the resource is not there. | No |
| Malformed response | The body did not decode, or was not the shape the protocol defines. | No |
| Contract / Provenance / Dependency / Graph / Impact / Snapshot / Report | A domain failure, each named rather than collapsed into one variant. | No |

An absence is deliberately not an error and not an empty result: it is its own outcome, so
that "the endpoint has no such entry" never becomes "the contract depends on nothing".

## Where the specification is consulted

`amasario-core`'s relationship vocabulary, basis ceilings, confidence ordering and
verification statuses are read from the specification's taxonomies, and the conformance
tests compare the engine's vocabularies against the specification checkout named by
`AMASARIO_SPEC_DIR` (or `.amasario-spec` in the repository root). CI clones the
specification to that path, so a taxonomy change on the specification side fails this
repository's build instead of silently altering an analysis.

## What is not here

No frontend, no wallet, no block explorer, no token, no security scanner. The engine's
interface is the CLI and the reusable Rust libraries behind it.

One qualification, because the list above is about the engine's workspace and this
repository is wider than its workspace: `reference-contract/` holds a pair of Soroban
contracts that no crate here links against, and the pair is deployed to Testnet so that the
dependency analysis has a target this project owns rather than only one it borrows. It is
not part of the stack - the deployment is made by a script outside the workspace, which
names an identity the `stellar` CLI holds, so no crate here signs anything. It used to say
"no on-chain contract" outright, which stopped being true the day the pair was deployed; see
[testnet.md](testnet.md) for the deployment itself and [security.md](security.md) for what
the engine does and does not claim.
