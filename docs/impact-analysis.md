# Impact analysis

Impact answers: **if this entity changes, what could that reach?**

```bash
amasario impact --contract CAAAA...D2KM --network testnet
amasario impact --contract CAAAA...D2KM --network testnet --depth 2
```

## Direction is declared, never inferred

The specification declares, per relationship, which way a change travels. The engine reads
that declaration and never infers a direction from a relationship's name.

| Relationship | Change travels | Why |
| --- | --- | --- |
| `DEPENDS_ON` | object → subject | A change to the dependency reaches the thing that requires it. |
| `INVOCATES` | object → subject | A change to the callee can break the caller. |
| `BUILT_FROM` | object → subject | A change to the source reaches the build. |
| `DERIVED_FROM` | object → subject | A change to the origin reaches what was derived. |
| `DEPLOYED_AS` | object → subject | A change to the deployment reaches the artifact. |
| `AFFECTS` | subject → object | The arrow already points the way the change travels. |
| `OBSERVED_IN` | *nothing* | Re-observing a fact does not change the fact. |
| `VERIFIED_BY` | *nothing* | A re-verification is not a change to the thing verified. |

The last two are the reason the direction is a property of the relationship rather than of
the traversal. If it were inferred from shape, `OBSERVED_IN` would look exactly like
`DEPENDS_ON` and a re-observation would be reported as a change.

Walking with the arrow along `DEPENDS_ON` is the mistake that produces a plausible, wrong
answer: it reports a change to a library as affecting the things that library requires,
which is the opposite of the truth. The fuzz targets check every step an analysis took
against the direction its relationship declares, in both directions — see
[ci-integration.md](ci-integration.md).

## A finding

A finding names everything a reader needs to disagree with it.

| Field | Meaning |
| --- | --- |
| Changed entity | What the change starts at. |
| Affected entity | What the change reaches. |
| Relationship path | The steps traversed, each with its endpoints, relationship, direction and evidence. |
| Hop depth | How many steps. |
| Impact type | The classification. |
| Change type | What kind of change was declared. |
| Confidence | Capped by the weakest step's basis. |
| Reason | Why the finding exists, in terms of the route. |
| Verification state | How well the route is supported. |

A finding one or more hops away always carries its path. A hop count without a route is an
assertion with nothing behind it: the reader cannot see which edges the claim rests on, and
therefore cannot disagree with it. The `impact` integration suite asserts that a
multi-hop finding has a path, that the path's step count equals the hop depth, that the
path names one more entity than it has steps, and that the finding says which way the
change travelled.

Every step carries the evidence of the edge it traversed, and a step is built only from an
edge that has some. A path therefore cannot contain an unsupported link.

## The corpus, which is one graph under four bounds

| Fixture | Change starts at | Bound | Reaches |
| --- | --- | --- | --- |
| `direct.json` | `bravo` | 1 hop | `alpha` |
| `transitive.json` | `charlie` | 2 hops | `bravo`, then `alpha` |
| `multi-hop.json` | `delta` | 4 hops | `charlie`, `bravo`, `alpha` |
| `bounded.json` | `delta` | 2 hops | `charlie`, `bravo` — and says it was bounded |

The change always starts at the *object* end of a chain, because that is where a change
reaches something. Starting at the subject would produce an analysis that finds nothing,
which is why the fixtures are arranged this way: a change to a caller does not travel to
what it calls.

`bounded.json` is the fixture worth having. It is the same graph as `multi-hop.json` under
a bound that cannot reach the end of the chain, so the two differ only by the bound — and
the difference is that one discloses it stopped. An analysis that silently stopped would
report a smaller affected set than exists, and a reader would have no way to tell that from
a contract with no further dependents.

## Termination

```
alpha ──▶ bravo ──▶ charlie ──▶ bravo   (a cycle)
```

An unbounded traversal over a cyclic graph does not terminate. The bounds are therefore
part of the safety story rather than a tuning parameter, and the cyclic graph is a fixture
rather than an edge case: the analysis reports the findings it reached, and the cycle is
available to the graph layer as a fact about the topology. See
[graph-analysis.md](graph-analysis.md).

## What impact analysis is not

A finding says that a change *could* reach an entity along a route the evidence supports.
It does not say the entity is broken, that it will behave differently, or that anyone
should act. It is a statement about the dependency structure and the direction the
specification assigns to each relationship — nothing more. In particular:

* an affected entity is not a vulnerable entity;
* a long path is not a strong conclusion — the aggregated confidence is the weakest step
  on the route, and a route through a `LOW_CONFIDENCE` inference is a weak finding however
  long it is;
* a cycle does not mean either contract is wrong; mutual recursion is a legitimate design.

See [security.md](security.md).

## Scale

Propagation is bounded twice, and the case worth measuring is the bound being *reached*: a
run that stops at its limit has done the same work as one that exhausted the graph.
[The benchmarks](../benches/impact-analysis.rs) measure the corpus's four kinds, the same
analysis over a pre-assembled graph (so the traversal and the assembly can be told apart),
a fan where the node bound is what runs out, and the cyclic graph, where the assertion
inside the measurement is that the analysis concluded rather than only that it returned.
