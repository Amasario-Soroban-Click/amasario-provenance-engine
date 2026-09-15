# Dependency analysis

A dependency is a claim, and a claim needs a basis and evidence. The engine's dependency
layer exists to enforce that: it takes observations, decides what the specification
permits them to become, and publishes the remainder as refusals rather than as silence.

```bash
amasario dependencies --contract CAAAA...D2KM --network testnet --depth 5
amasario dependencies --contract CAAAA...D2KM --network testnet --format json --pretty
```

## The four stages

```
observations → candidates → classification → resolution → a validated set
```

1. **Detection.** Recorded invocations, operations and events become *candidates*. A
   candidate is not a dependency: it is an observation with a basis and citations.
2. **Classification.** Each candidate is checked against the rules. A candidate whose
   relationship does not permit its endpoints, or whose basis cannot establish the claim
   it makes, is refused here.
3. **Resolution.** The survivors are deduplicated, sorted canonically, assembled into one
   set and validated against the partition, evidence and reason rules.
4. **Closure.** The subject's direct edges are walked, bounded, so that what it reaches
   *through* an intermediate becomes a transitive entry with the path that establishes it.

The naming matters. `Candidate` rather than `Dependency` at stage one is what stops a
refusal from being recorded as an edge, and it is why the refusal has somewhere to go.

## What may be a dependency

| Relationship | Direction of change | Can it be a dependency? |
| --- | --- | --- |
| `DEPENDS_ON` | object → subject | Yes |
| `INVOCATES` | object → subject | Yes |
| `BUILT_FROM` | object → subject | Yes |
| `DERIVED_FROM` | object → subject | Yes |
| `DEPLOYED_AS` | object → subject | Yes |
| `AFFECTS` | subject → object | Yes, as an impact claim rather than a requirement |
| `OBSERVED_IN` | none | No |
| `VERIFIED_BY` | none | No |

The direction is declared by the specification, never inferred from a relationship's name.
`DEPENDS_ON` and `VERIFIED_BY` look structurally identical and behave in opposite ways;
one of them does not propagate at all.

## Basis, and how much a claim can support

A dependency's confidence is capped by the basis that established it.

| Basis | Ceiling | Meaning |
| --- | --- | --- |
| `OBSERVED_INVOCATION` | `VERIFIED` | A cross-contract call was recorded in a successful transaction. |
| `OBSERVED_EVENT` | `VERIFIED` | The same, observed in a contract event. |
| `RESOLVED_LOCKFILE` | `HIGH_CONFIDENCE` | A lockfile resolved the package. |
| `EMBEDDED_DIGEST` | `HIGH_CONFIDENCE` | A recorded digest agreed with a computed one. |
| `DECLARED_MANIFEST` | `MEDIUM_CONFIDENCE` | A manifest declared it. |
| `CONFIGURED_ENDPOINT` | `MEDIUM_CONFIDENCE` | An endpoint was configured with it. |
| `ATTESTED` | `HIGH_CONFIDENCE` | An attestation covers it. |
| `INFERRED_INTERFACE` | `LOW_CONFIDENCE` | Interface similarity suggests it. |

Only an observation reaches `VERIFIED`. A manifest that declares a dependency is not
evidence that the dependency is used, and an interface that looks compatible is consistent
with a requirement without being evidence of one. A claim at `INFERRED_INTERFACE` is
labelled, capped, and kept out of the observed facts a report lists.

## What is never enough on its own

A dependency is **not** asserted because:

* two repositories share a name;
* two projects mention one another;
* two contracts exist in the same ecosystem;
* two packages have similar metadata;
* an interface looks compatible.

Each of those is a reason to look, not a reason to claim. The engine has no code path that
turns any of them into an edge.

## The two partitions

A dependency set is split in two, and an edge belongs to exactly one.

* **Direct** — the subject itself reaches it in one hop. Its `depth` is 0 and its path is
  empty.
* **Transitive** — the subject reaches it only through at least one intermediate. Its
  `depth` is at least 2, and it carries the intermediate entities in order.

The partition rule is not a presentation choice. It is what makes a transitive entry
readable: an entry with no path would be indistinguishable from a direct edge one hop
further out, and a reader could not tell whether the analysis walked a real chain or
collapsed two uncertain hops into one confident-looking statement.

The rule is checked in three places: `resolve` refuses a set that violates it, the
document projection exposes `partition_violations()` for a consumer to re-check, and the
`dependencies` integration suite asserts it over every committed fixture.

Fixtures: `fixtures/dependencies/direct.json` (two direct edges),
`transitive.json` (a direct invocation closed into a two-hop chain), `mixed.json` (three
direct edges — from an operation, from an event, and from a call the ledger reported as
failing — alongside a refusal), `refused.json` (one call whose outcome was not reported).

## The closure, and what it discloses

```
alpha ──INVOCATES──▶ bravo ──INVOCATES──▶ charlie ──INVOCATES──▶ delta
```

From `alpha`, `charlie` is a transitive dependency at depth 2 with path `[bravo]`, and
`delta` is one at depth 3 with path `[bravo, charlie]`.

The walk is bounded by hops and by nodes, and the result says which bound it hit. A
closure that stopped early sets `truncated` and a `truncationReason`; a result carrying one
without the other is refused. That is the difference between "the contract depends on
nothing further" and "the analysis stopped", and no reader can tell them apart from a short
list.

The aggregated confidence of a transitive entry is the **minimum** among the hops it
traverses. A chain is only as strong as its weakest link, and taking the strongest hop
would let confidence be manufactured out of unrelated evidence.

Cycles are reported with the edges that form them rather than removed or collapsed. A
consumer that never sees the cycle cannot know the graph was not a tree.

Fixtures: `fixtures/graphs/transitive.json`, `multi-hop.json`, `cyclic.json`.

## Refusals are published

A candidate the rules refuse becomes an `unresolved` entry with a reason and the engine's
explanation, not a dropped observation.

```json
{
  "unresolved": [
    {
      "source": { "kind": "CONTRACT", "id": "CCQ2...CNSG" },
      "reason": "NOT_PERMITTED",
      "detail": "INVOCATES CONTRACT:CDS6...KCXD is not permitted: the endpoint did not report whether the invocation succeeded, and an unknown outcome is not a successful one; observed: alpha called mint on the contract at 0xe5; basis OBSERVED_INVOCATION"
    }
  ]
}
```

This is the refusal `fixtures/dependencies/mixed.json` publishes. `refused.json` publishes
a longer one, for a call whose outcome was never reported at all, and its text names the
rule that was applied: a `RUNTIME` dependency must rest on a transaction recorded as
successful, and an unknown outcome is not a successful one.

Dropping the refusal would be the worse option. A consumer that saw an empty `edges` array
would conclude the contract depends on nothing, when the engine had been told the contract
invokes something and refused to call it a dependency. `NOT_PERMITTED` is the schema's own
word for "the specification's rules did not permit this claim", and `detail` carries the
rest, so a self-dependency and a failed classification read as two different explanations
of one reason rather than as one undifferentiated gap.

## Scale

A contract's own dependency set holds only *its* observations. A second hop belongs to the
contract that made the call, so the closure is walked over the union of the observations
the analysis collected, not over the subject's set alone. That is what the pipeline does,
and it is why `dependencies --depth 5` can report a chain longer than the subject's own
history.

Each hop is being asked of a real chain, so the cost is not linear in the subject's edges.
[benchmarks](../benches/dependency-resolution.rs) measure resolution, closure and merge
separately, and the closure up to the engine's own depth ceiling. See
[ci-integration.md](ci-integration.md) for running them.
