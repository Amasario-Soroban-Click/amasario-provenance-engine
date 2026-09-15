# Graph analysis

The dependency graph is the structure the rest of the engine reasons over. It is a typed,
directed graph whose edges carry evidence, confidence and an observation boundary, and
whose identifiers are derived rather than assigned.

```bash
amasario graph --contract CAAAA...D2KM --network testnet --format dot -o graph.dot
amasario graph --contract CAAAA...D2KM --network testnet --format json --pretty
```

## Nodes

| Kind | What it is |
| --- | --- |
| `CONTRACT` | A deployed contract, identified by its address. |
| `WASM` | An executable module, identified by its digest. |
| `SOURCE` | A repository revision. |
| `BUILD` | A build of a source revision. |
| `ARTIFACT` | A produced artifact. |
| `DEPLOYMENT` | A deployment and the transaction that performed it. |
| `PACKAGE` | A package a build used. |
| `TRANSACTION` | A ledger transaction, as an observer rather than as a dependency. |

A node's identifier is its kind and its own identifier — `CONTRACT:CCQ2...CNSG`,
`WASM:0101...0101`. The kind is in the identifier so that a node read out of a document
cannot be silently re-typed.

## Edges

| Relationship | Direction | Change propagation |
| --- | --- | --- |
| `DEPENDS_ON` | subject requires object | object → subject |
| `INVOCATES` | subject called object | object → subject |
| `BUILT_FROM` | subject was built from object | object → subject |
| `DERIVED_FROM` | subject derives from object | object → subject |
| `DEPLOYED_AS` | subject is deployed as object | object → subject |
| `AFFECTS` | subject affects object | subject → object |
| `OBSERVED_IN` | subject was observed in object | none |
| `VERIFIED_BY` | subject is verified by object | none |

Every edge carries: its source and target, its relationship, at least one evidence
citation, a confidence with its own citations, and the observation boundary. An edge
without a citation cannot be added, so an unsupported claim cannot appear in a graph.

### Edge identifiers

An edge's identifier is a digest over its endpoints and its relationship. It is not a
counter and not a position, and that is deliberate: two analyses of the same contract at
two boundaries must agree on which edge is which, or every diff would report every edge as
replaced.

The graph layer and the dependency-document layer compute the same identifier, and the
`graphs` integration suite asserts it by finding each graph edge's identifier in the
matching dependency document. The two documents name an endpoint differently on purpose —
a graph document carries `KIND:id`, a dependency document carries a structured reference —
so the comparison goes through the identifier, which is what the two layers must agree on.

### What the graph refuses

| Attempt | Result |
| --- | --- |
| An edge whose endpoint is not a node | Refused. The endpoint is not invented; dangling edges are not repaired. |
| A transitive dependency as an edge | Refused. A transitive dependency is a path, not a hop. |
| Two edges with the same endpoints and relationship | Refused. A stable identifier is what lets a diff tell an unchanged edge from a replaced one. |
| A self-dependency | Refused before the graph, by candidate construction. |

## Traversal

Four traversals, four questions.

| Function | Question |
| --- | --- |
| `walk` | What does this entity reach? The dependency question. |
| `walk_reverse` | What reaches this entity? The impact question. |
| `all_paths_bounded` | By what routes? What an impact finding cites. |
| `find_cycles` | Is the answer a tree at all? |

`walk_reverse` is not `walk` with the edges reversed by accident: the two directions visit
different subgraphs on any graph that is not a chain, which is why both are measured
separately in [the benchmarks](../benches/graph-traversal.rs).

### Bounds

A traversal takes a depth bound and a node bound. `MAX_PERMITTED_DEPTH` is 32; a larger
configured bound is clamped rather than refused, and the bound actually used is recorded in
the result. A traversal that stopped early reports `truncated` **and** a
`truncationReason`; a result carrying one without the other is refused.

A walk that reached its bound is inconclusive. That is a property of the result rather
than of the caller's interpretation, so `is_inconclusive()` answers it rather than leaving
a reader to infer it from a node count.

### Cycles

A cycle is reported, never collapsed and never silently removed.

* `has_cycle` answers the boolean.
* `find_cycles` names the cycles and the edges that form them.
* A cycle's entities number at least two: the graph refuses a self-dependency, so a
  one-entity cycle would mean an edge from a node to itself had been admitted.

The two implementations are one question asked twice, which makes them the pair most
likely to drift, so the fuzz target asserts they agree on every generated graph — see
[ci-integration.md](ci-integration.md).

Fixture `fixtures/graphs/cyclic.json` holds a real cycle: `charlie` invokes `bravo` and
`bravo` invokes `charlie`, which is what mutual recursion between two contracts looks
like. `fixtures/graphs/disconnected.json` holds a contract no edge reaches, because a
graph whose every node lies on a path never exercises the disconnected case.

## Determinism

Two builds of one graph produce the same bytes. Nodes are ordered by kind then identifier,
edges by the same rule the resolver uses, and the document's metadata records the node
count, the edge count, whether the graph is cyclic and which nodes are disconnected. The
`graphs` suite asserts byte equality across two builds of every fixture, which is what
makes a committed fixture meaningful: a diff under `fixtures/` is a change to the model
rather than to a file.

## Fixtures

| Fixture | Shape |
| --- | --- |
| `direct.json` | One subject, two outgoing edges. |
| `transitive.json` | A two-hop chain. |
| `multi-hop.json` | A four-entity chain. |
| `cyclic.json` | A chain that closes back on itself. |
| `disconnected.json` | A graph holding a contract no edge reaches. |

## Scale

Traversal is bounded, but a bound is not a performance guarantee: a correct bound that is
quadratic is still a bound a user experiences as a hang. The benchmarks measure walks over
a chain at every depth up to the engine's ceiling, and over a breadth-first tree where the
node bound rather than the hop bound is what runs out. See
[ci-integration.md](ci-integration.md) for how to run them.
