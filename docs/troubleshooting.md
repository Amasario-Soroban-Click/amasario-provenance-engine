# Troubleshooting

Most confusions about this engine come from reading a bounded or failed result as a
final one. This page starts with the exit codes, because they are the fastest way to
tell which of those two things happened, and then works through the failures that
actually occur.

## Exit codes

A run exits zero only when it completed **and** passed any gate it was asked to
enforce. Everything else is a distinct code, so a CI job can branch on the outcome
without parsing a message.

| Code | Meaning |
| --- | --- |
| `0` | The run completed and every requested gate held. |
| `1` | The run completed and a gate did not hold (`EXIT_GATE_FAILED`). |
| `2` | A usage or configuration problem (`EXIT_USAGE`). Matches clap's own code. |
| `65` | `EX_DATAERR`: `PROVENANCE`, `DEPENDENCY`, `GRAPH`, `IMPACT`, or `SPECIFICATION_COMPATIBILITY`. |
| `66` | `EX_NOINPUT`: `CONTRACT` or `SNAPSHOT`. A contract or snapshot that could not be read or found. |
| `69` | `EX_UNAVAILABLE`: `NETWORK`. |
| `70` | `EX_SOFTWARE`: `REPORT`, `EXPORT`, or `INTERNAL`. |
| `74` | `EX_IOERR`: writing output, or (de)serialising a document, failed. |

The two codes that matter most in a pipeline are `1` and `69`. Code `1` means the
engine did its job and the answer is no — for example `verify --require verified` ran
a complete verification and it did not come out `VERIFIED`. Code `69` means the
engine could not do its job, and the result says nothing about the contract. A script
that treats both as "failed" will retry a genuine `CONFLICTING`, or worse, accept a
network outage as a clean result.

`2` is refused before any analysis begins. A bound outside the range the specification
permits — a zero `--depth`, for instance — is a configuration error, not a short
search, so it fails with a named problem rather than producing a smaller answer that
looks complete.

## A result is shorter than expected

Look for the truncation reason before assuming there was nothing to find. The engine
distinguishes "the search stopped" from "there was nothing left", and the reason is
carried in the result rather than only in a log line.

| Reason | What happened | What to do |
| --- | --- | --- |
| `MAX_DEPTH_REACHED` | Traversal crossed `--depth` edges and stopped. | Raise `--depth` if the graph is genuinely deeper, or accept the bound and report it. |
| `MAX_NODES_REACHED` | `--max-nodes` entities were accumulated. | Raise `--max-nodes`, or narrow to one contract. |
| `EVIDENCE_UNAVAILABLE` | A relationship could not be established because the observation that would support it was not reachable. | Widen the observation boundary, or use an endpoint that serves the required ledgers or transactions. |
| `BOUNDARY_REACHED` | Continuing would require observing beyond the boundary the run was given. | This is the boundary working as intended; move the boundary if you need more history. |
| `RATE_LIMITED` | The endpoint returned 429 and the retry budget was spent. | Lower concurrency, wait, or run against your own RPC node. This is the reason most likely to be mistaken for a complete result, which is why it has its own value. |
| `CANCELLED` | The operation was cancelled before it completed. | Re-run; this is transient in a way a bound is not. |
| No truncation, empty dependencies | The search completed and found no edge with evidence. | This is an answer, not a failure. Compare with `--scan-events`, below. |

Only `RATE_LIMITED` and `CANCELLED` are transient. A depth or node bound is a
deliberate limit, so a later run reaches the same point unless the bound changes -
which is a distinction a consumer deciding whether to re-run needs and cannot derive
from the reason's name alone.

A truncated traversal still returns what it found. It does not error, and it does not
pretend the found set is closed. If your consumer treats an empty or short list as
authoritative, it will be wrong exactly when the network is busy.

## No dependencies found, but the contract clearly calls others

Two causes are common, and they are distinguishable.

**Events were not scanned.** Cross-contract calls are observed in contract events. The
default does not scan them, because event history is large and most runs do not need
it. Add `--scan-events`:

```console
amasario dependencies --contract CABC... --network testnet --scan-events
```

**The evidence was not reachable.** If the deployment transaction is outside the
ledger range your endpoint serves, or the RPC node does not have it, the engine has no
observation to build an edge on. It reports truncation, not an empty result. A
dependency is never inferred from two contracts sharing a name, being mentioned
together, or existing in the same ecosystem — an edge without evidence is refused.

## `NETWORK` errors

`amasario-network` classifies rather than merges failures, and only transient ones are
retryable. A timeout, a rate limit or an unavailable endpoint is retried; a malformed
response is **not**, because retrying it produces the same malformed response and
hides a real defect behind a delay.

- `429` — rate limited. The retry budget is finite and then the run reports
  `rateLimited` truncation or fails with `NETWORK`. Lower concurrency or use your own
  node.
- Malformed or unexpected response — the endpoint is not speaking the protocol the
  engine expects, or a proxy is rewriting the body. Not retryable by design.
- Unreachable host — check the endpoint and any egress restrictions.

The engine never converts a network failure into an empty dependency set. If you are
seeing an empty result rather than a `NETWORK` failure, the request succeeded and the
answer really is empty; look at truncation instead.

## `CONTRACT` failures

| Symptom | Cause |
| --- | --- |
| Invalid contract id | The strkey failed its checksum or shape check. A Soroban contract id starts with `C`. |
| No such contract | The address is well-formed but the network has no such contract at or below the observation boundary. |
| Not an executable | The address exists but does not host a deployed WASM module. |

A failure to inspect says nothing about provenance. It is not a `CONFLICTING`
verification; there was nothing to verify against.

## Provenance is not `VERIFIED`

Read the status, because each one means something specific and only one is a
contradiction.

- `UNVERIFIED` — the claim could not be evaluated. Commonly, no source revision or no
  build configuration was available, so there was nothing to match.
- `PARTIALLY_VERIFIED` — some of the chain matched: for example the source revision
  rebuilt to the right digest, but the deployment could not be tied to the artifact.
- `CONFLICTING` — evidence actively contradicts the claim. A claimed source revision
  that rebuilds to a different digest from the deployed module is the canonical case.
- `UNKNOWN` — nothing was claimed.

`CONFLICTING` takes precedence over every other status, and the engine cannot report
`VERIFIED` when it holds. If your pipeline compares the deployed digest directly, make
sure you compare against the **deployed executable hash**, not the source artifact
digest — they are different identities, and the model deliberately distinguishes them.

## A snapshot is rejected as incompatible

Snapshots carry the specification version that produced them and are refused when the
reading engine cannot interpret that version. A version mismatch reports
`SPECIFICATION_COMPATIBILITY` (exit `65`) rather than being interpreted anyway,
because silently reinterpreting a changed field is how a comparison produces a wrong
answer with no indication that it did.

Compare snapshots produced by compatible specification versions, or regenerate the
older snapshot with the current engine. A snapshot whose declared volatile fields
changed — timestamps, for instance — does not compare as changed; the content digest
excludes them, so two captures of an unchanged subject agree.

## A graph reports a cycle, or refuses an edge

Cycles are detected rather than followed forever, and they are reported as a property
of the graph, not an error. A cyclic dependency is a legitimate finding.

An edge that cannot be resolved — a broken reference, a duplicate edge, an impossible
relationship — is refused with the offending edge named. When a dependency candidate is
refused by a rule, the engine publishes it as an unresolved candidate with the reason
(for example `NOT_PERMITTED`) instead of dropping it, so a refusal is visible rather
than appearing as an absence.

## Report or export fails to represent something

`REPORT` and `EXPORT` fail (exit `70`) when a requested rendering cannot represent the
content — DOT and GraphML have no way to express a nested evidence record, for example,
so the evidence is referenced by identity. The document is not silently trimmed. JSON
is the lossless rendering; use it when a downstream consumer needs everything.

## `cargo` and CI problems

**The conformance tests are skipped.** The specification-conformance tests in
`amasario-core` are `#[ignore]`d by default because they read a checkout of the
normative specification, and a suite that fails without an unrelated checkout is one
people stop running, so they are opt-in:

```console
AMASARIO_SPEC_DIR=../amasario-provenance-spec cargo test -p amasario-core -- --include-ignored
```

Without `AMASARIO_SPEC_DIR`, those tests do not run at all rather than failing.

**Clippy refuses `HashMap`.** This is `clippy.toml`, not a mistake. Output-bearing
collections are ordered so that results are deterministic; `HashMap` and `HashSet` are
refused by name.

**A test needs the network.** The integration suites run against a recorded corpus and
do not reach the live network. CI is not flaky because a public endpoint was busy.

## Reading a failure message

Every engine error carries a category, so start there rather than at the text. The
category tells you which subsystem refused and whether a retry is meaningful; the
message tells you which input caused it. If a failure has category `INTERNAL`, it is a
defect in the engine, not in your input, and the message is written to be reported as
such.
