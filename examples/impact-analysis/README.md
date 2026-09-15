# Example: what a change could reach

Given `A → B → C`, a change to `C` may affect `B` and `A`. This example is the surface
that propagation runs over.

## The command

```console
cargo run -q -p amasario-cli -- export \
  --input fixtures/graphs/multi-hop.json \
  --format dot
```

Runs offline. The document is a four-entity chain produced by the engine, so the
propagation surface below is the engine's own graph rather than an illustration of one.

## What the output shows

Four contracts in a line, with three evidenced edges. Reading it from the tail:

```
CCQ2...  →  CCZL...  →  CDB4...  →  CDKN...
```

A change to `CDKN...` concerns three entities: `CDB4...` at one hop, `CCZL...` at two,
and `CCQ2...` at three. The direction is what makes the answer, and it is why impact is
computed over the dependency direction rather than over an undirected neighbourhood.

## What an impact record contains

The finding set - the shape of what `amasario impact` produces - is in
`fixtures/impact/multi-hop.json`, and each entry names more than the entity:

| Field | Why it is there |
| --- | --- |
| changed entity | What the analysis started from, so a finding is never an orphan |
| affected entity | What could be reached |
| relationship path | The route, so a reader can check the reasoning rather than the conclusion |
| hop depth | How far the effect travelled, which is the difference between a direct and a distant concern |
| impact type | Which semantics apply to this route |
| evidence, confidence | The same requirement as everywhere else: no finding without evidence |
| reason | Why this route counts as an impact at all |
| verification state | Whether the route's evidence was verified, and how well |
| change type | `MODIFIED`, `REMOVED` and the rest, because a removed dependency and a modified one do not propagate alike |

## Bounded, and honest about it

`fixtures/impact/bounded.json` is the same analysis under a bound that stops it early:
two findings, deepest at two hops, and `truncated` set. A bounded impact analysis that
reported two findings without saying it was bounded would be the most dangerous output
this engine could produce - it would look complete, and it would be short.

That is why the truncation is a field in the result rather than a log line.

## The live equivalent

```console
cargo run -q -p amasario-cli -- impact \
  --contract CCQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2CNSG \
  --network testnet --change-type MODIFIED
```

`--change-type` is required, because "what could be affected" has no answer without it. A
change that removes a dependency propagates differently from one that modifies it, and
an engine that assumed a default would be answering a question nobody asked.

## What impact analysis does not mean

A finding means an entity **could be affected**, on the evidence and within the
boundary. It is not a prediction, and it is not a severity. Nothing in the impact model
carries a score, and no finding means a contract is vulnerable - only that a change
upstream of it is worth a look.
