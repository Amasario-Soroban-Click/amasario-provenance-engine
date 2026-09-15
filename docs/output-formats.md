# Output formats

The engine writes one document per concept, and it writes the shape the specification
defines for it. Where a schema exists in `amasario-provenance-spec`, that schema is the
shape; where a rule cannot be expressed in a schema, the engine checks it in code and the
conformance tests assert it.

## Report renderings

`amasario report --format <FORMAT>` and every command that can render a report:

| Format | Consumer | What it is |
| --- | --- | --- |
| `text` | A terminal | A human-readable summary. The default. |
| `json` | A pipeline | The specification's report document, canonically written. |
| `markdown` | A person | The same content as a file, with the sections kept apart. |
| `dot` | A graph renderer | The graph the report was assembled from. |
| `junit` | A test reporter | XML a CI job can gate on. |

### JSON

Canonical means: the field order is the schema's, the section arrays are in the order they
were added, and an absent optional field is omitted rather than written as `null`, so that
"not established" and "established as empty" cannot be confused.

Nothing is added for convenience. `report.schema.json` sets `additionalProperties: false`,
so an extra field makes the document invalid — which is how an engine comes to emit
documents that consumers refuse.

A report the engine holds in memory can carry a graph so that DOT and Markdown can be
rendered from it, but the document has no graph field and one is never written. The
`reports` integration suite asserts both halves: that the rendering round-trips, and that
`"graph"` does not appear in the output.

### Markdown

The four sections are kept apart, in this order:

1. **Observed** — what the endpoint returned at the boundary, with citations.
2. **Inferred** — what follows from observations, each with the basis it was inferred by.
3. **Verification** — each link or claim with its status and the gap that remains.
4. **Unknown** — the questions the engine could not answer, with the reason.

The `unknown` section is the one that matters most. A report that omitted its own
incompleteness would read as a complete account of a contract, when it is an account of one
address at one ledger.

### DOT

Requires a report that carries a graph. A report document has no graph field, so a report
read back from JSON cannot be drawn — and the renderer refuses rather than emitting an
empty `digraph`, because an empty digraph reads as a finding about the topology rather than
as the absence of one.

### JUnit

A JUnit document has four things a case can be: passed, failed, errored, skipped. The
specification has five verification statuses. The mapping states what a build should do
rather than compressing the statuses into a number:

| Status | JUnit | Why |
| --- | --- | --- |
| `VERIFIED` | passed | The evidence is consistent with the claim. |
| `PARTIALLY_VERIFIED` | passed, gap in the output | Part of the claim holds; a build need not stop. |
| `CONFLICTING` | failed | The claim is contradicted, which is the one outcome that must stop a build. |
| `UNVERIFIED` | failed | Nothing supports the claim, and treating that as a pass is how an unverified artifact ships. |
| `UNKNOWN` | skipped | The question could not be asked, which is neither a pass nor a failure. |

`CONFLICTING` failing the build is the important one. It is the status that exists because
a claimed source revision that rebuilds to a different digest must not be reported as
verified, and the same reasoning holds for a CI gate.

A report's `errors` section maps onto `<error>`, not `<failure>`. "The network did not
answer" and "the answer was that the claim is contradicted" are different findings — the
distinction the whole error model exists to preserve.

The document is one `<testsuites>` wrapper holding one `<testsuite>`.

## Documents

| Command | Document | Specification schema |
| --- | --- | --- |
| `inspect` | A contract inspection as JSON | `contract.schema.json`, `wasm.schema.json` |
| `discover` | The observed company, as JSON | — |
| `provenance` | A provenance chain as JSON | `provenance.schema.json` |
| `dependencies` | A dependency set document | `dependency-set.schema.json` |
| `graph` | A graph document | `graph.schema.json` |
| `impact` | A finding set | `impact.schema.json` |
| `snapshot create` | A snapshot, as JSON | `snapshot.schema.json` |
| `snapshot show` | The snapshot re-rendered | as above |
| `diff` | A diff document | `diff.schema.json` |
| `verify` | A verification outcome | — |
| `report` | A report document | `report.schema.json` |
| `export` | A recorded analysis | — |

A dependency set document carries the direct edges, the transitive edge identifiers and,
in `unresolved`, every candidate a rule refused. See
[dependency-analysis.md](dependency-analysis.md).

## Export formats

`amasario export --input <FILE> --format <FORMAT>` reads a snapshot or a graph document and
writes one of:

| Format | For |
| --- | --- |
| `json` | A pipeline, and the format that preserves everything. |
| `yaml` | A person editing a document by hand. |
| `graphml` | Graph tools that speak neither JSON nor DOT. |
| `dot` | Graphviz. |

An export preserves enough to reconstruct the graph and its evidence references: every
edge keeps its identifier, its relationship, its confidence and its citations, so an
exported graph is not a lossy drawing of the analysis.

## Field naming

Every document uses the specification's field names, including its camelCase, and every
struct that reaches output declares `deny_unknown_fields` on the way in. A document the
engine writes is a document the engine can read, and the `verification` suite asserts it
over the whole fixture corpus: each file parses with the type's own deserialiser, not only
with a generic JSON reader.

## Determinism

Two runs over the same evidence write the same bytes. Ordering is canonical everywhere,
identifiers are derived rather than assigned, and the integration suites assert byte
equality across two builds of every fixture — so a diff under `fixtures/` means the model
changed.
