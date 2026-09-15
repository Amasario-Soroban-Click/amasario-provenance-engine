# Example: one subject in every output format

Everything the engine establishes about one contract, captured at one boundary, rendered
in each format and read back. This is the example to run when integrating the engine into
a pipeline, because it is the one that shows where each rendering loses information.

## The command

```console
bash examples/complete-analysis/command
```

The script reads `fixtures/snapshots/testnet-alpha-after.json` - a snapshot holding the
contract's identity, module identity, provenance links, dependency graph, evidence,
confidence and impact surface - and renders it five ways:

| Rendering | What it is for |
| --- | --- |
| `snapshot show` | The summary a person reads first: subject, boundary, counts, digest |
| `export --format json` | The lossless form, for a consumer that needs everything |
| `export --format yaml` | The same document, for a reviewer who has to read the diff |
| `export --format dot` | Graphviz, for a picture with evidence IDs on the edges |
| `export --format graphml` | The format graph tooling consumes |

## What to notice

**The three identity fields are separate and stay separate.** The snapshot's subject is a
contract; `engine 1.0.0` is what produced the document; the digest is over the content,
not over the contract. Reading the wrong one is a category error, so none of them is
abbreviated to "the id".

**`--format` on `export` accepts a snapshot or a graph document, not any document.** A
provenance chain or a dependency set is not an export format's input; passing one is a
usage error naming both, which is why the error text says *neither a snapshot nor a graph
document* and prints what it found instead.

**DOT and GraphML cannot express an evidence record.** They carry the edge's evidence
identity as an attribute, so the picture and the JSON agree on what supports each edge,
and the JSON is where the record's contents live. The export does not silently trim;
what cannot be represented is represented by reference.

**JSON is the only lossless rendering.** YAML is the same document for a human reader;
DOT and GraphML are projections. A pipeline that needs everything should read JSON and
render the rest itself.

## The shape of the whole thing

A snapshot is what a complete analysis leaves behind, and every section of it comes from
a later stage of the same pipeline:

```
CLI → configuration → target → observation → evidence → dependencies →
graph → verification → confidence → impact → snapshot → report → export
```

`docs/data-flow.md` walks that pipeline stage by stage. The snapshot is the point at
which the run becomes a document, and it records its own specification and engine
version so that a later reader can tell which contract of interpretation applies.
