# Example: a direct invocation closed into a transitive chain

A direct edge tells you what a contract calls. A transitive closure tells you what it
depends on, which is the question that matters when something upstream changes.

## The command

```console
cargo run -q -p amasario-cli -- export \
  --input fixtures/graphs/transitive.json \
  --format dot
```

Runs offline against the generated corpus.

## What the output shows

A two-hop chain: `CCQ2...` invokes `CCZL...`, which invokes `CDB4...`. Both edges are
individually evidenced, and the second is as much an observation as the first - a
transitive dependency is not a guess about the first contract, it is a fact about the
second.

That distinction matters when reading the result. The closure is computed from observed
edges, so the chain's length is a property of what was observed, not of how far the
engine looked. A chain that stops is either complete or bounded, and the result says
which: `metadata.truncated` is `false` here, meaning the traversal ran to its end rather
than to its `--depth` limit.

## Why the bound is not optional

`amasario dependencies --depth 5` bounds traversal for the same reason a recursive query
needs a limit: a dependency graph with a cycle has no natural end, and a closure over one
would not terminate. `fixtures/graphs/cyclic.json` is the corpus's cycle, and the engine
reports it as a property of the graph rather than failing or looping - a cyclic
dependency is a legitimate finding about a real contract graph, not an error.

`--max-nodes` bounds the other axis. A graph may be shallow and enormous, and a depth
limit alone does not bound the work.

## The live equivalent

```console
cargo run -q -p amasario-cli -- dependencies \
  --contract CCQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2CNSG \
  --network testnet --depth 2
```

Each hop costs network requests, so a deeper bound is not free. The engine pages through
transactions rather than issuing an unbounded recursive query, caches what it can within
a run, and reports `MAX_DEPTH_REACHED`, `MAX_NODES_REACHED` or `RATE_LIMITED` when it
stops early. Only `RATE_LIMITED` is transient: a later run reaches the same depth bound
at the same point unless the bound changes.
