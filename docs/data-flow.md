# Data flow

This page follows one analysis from the command line to an exported document. It is the
concrete version of the layer diagram in [architecture.md](architecture.md): what is read
at each step, what is produced, and what happens when a step cannot finish.

## The pipeline

```
CLI
 → load configuration
 → load the Amasario specification
 → validate the requested profile
 → resolve the target contract
 → identify the network
 → inspect the contract
 → collect evidence
 → resolve dependencies
 → build the dependency graph
 → verify provenance
 → calculate confidence
 → perform impact analysis
 → capture a snapshot          (when asked)
 → compare snapshots           (when asked)
 → generate a report
 → export machine-readable results
```

The stages are not a diagram: they are what `amasario-core`'s pipeline actually runs, and
the CLI calls the pipeline rather than reimplementing it. `amasario inspect` runs the
prefix that ends at contract inspection; `amasario impact` runs the whole thing and
reports the impact section; `amasario snapshot create` runs it and writes the snapshot.

## Stage by stage

### Configuration

Read from the command line. Two bounds, a depth and a node count, are always present;
their defaults are 6 and 10000. A bound of zero is refused rather than treated as a small
request, because a traversal that can visit nothing produces an empty answer that reads
like a complete one.

### Specification

The normative vocabulary comes from `amasario-provenance-spec`. Where the specification
has a machine-readable schema for an output, that schema is the shape the engine writes —
see [output-formats.md](output-formats.md). Where the specification states a rule in
prose that a schema cannot express (the partition rule, the evidence rule, the "a
truncated result must say why" rule), the engine checks it in code and the conformance
tests assert it against the specification's own text.

### Network identity

The endpoint's reported chain is compared against the passphrase the caller named. A
mismatch fails the run. This is why a network the engine has no published identity for
requires `--passphrase`: without one, the check would have to be skipped, and skipping it
is how a testnet analysis silently becomes a mainnet analysis.

### Contract inspection

The contract's identity, its executable and its module digest are read from the ledger.
What was observed is separated from what was inferred and from what could not be
obtained — see [contract-discovery.md](contract-discovery.md).

### Evidence collection

Transactions, operations, events and deployment records are read within the observation
boundary and become evidence records with a class and a citation — see
[evidence.md](evidence.md).

### Dependency resolution

Recorded invocations become candidates, candidates are classified against the rules, and
the survivors become a dependency set split into a direct and a transitive partition —
see [dependency-analysis.md](dependency-analysis.md).

### Graph assembly

The direct edges become a typed graph whose edges carry derived identifiers — see
[graph-analysis.md](graph-analysis.md).

### Provenance verification

The chain source → revision → build → artifact → WASM → deployment is assembled link by
link, and each link gets a verification status — see [provenance.md](provenance.md) and
[verification.md](verification.md).

### Confidence

Every claim that carries a confidence also carries the evidence it rests on. A confidence
with no citation cannot be constructed. The level is capped by the basis: an inference
from interface similarity cannot claim more than low confidence no matter how consistent
it looks.

### Impact

A change is propagated against the direction each relationship declares, within the
bounds. Findings name the changed entity, the affected entity, the route, the hop depth,
the change type, the evidence and the reason — see [impact-analysis.md](impact-analysis.md).

### Snapshot

A snapshot is the canonical form of everything above at one boundary, with a content
digest over it — see [snapshots.md](snapshots.md).

### Report and export

The report is the specification's report document, rendered as JSON, Markdown, DOT or
JUnit. Export writes a recorded analysis as JSON, YAML, GraphML or DOT — see
[output-formats.md](output-formats.md).

## Failure, and where it is allowed to stop

Every stage can fail, and each failure keeps its own identity instead of collapsing into
one generic error.

| Situation | Result |
| --- | --- |
| The endpoint does not answer | A transient network error. Retried with backoff, then reported. |
| The endpoint answers `404` | An absence. Not an error, not an empty result. |
| The body does not decode | A malformed-response error naming the endpoint and a truncated body. |
| The passphrase does not match | A configuration error. The run stops before anything is analysed. |
| A contract does not exist | Reported as absent. The run exits non-zero. |
| A candidate is refused by a rule | Published as an unresolved entry with a reason and the engine's explanation. |
| A traversal hits a bound | The result says it is truncated, and says which bound it hit. |
| A provenance link cannot be established | `UNVERIFIED` or `UNKNOWN`. Never `VERIFIED`. |
| A claimed revision rebuilds to a different digest | `CONFLICTING`. Never `VERIFIED`. |
| A report cannot be rendered in a format | A refusal that says why (for example, DOT for a report that carries no graph). |

## Exit codes

`0` means the command produced its result. Non-zero means it did not, the reason is on
stderr, and the code names the category so a script can branch without parsing text.
`amasario verify` is the gate: it exits `1` when the evidence refutes the executable's
identity or falls short of `--require`. The full table is in [cli.md](cli.md), and how a
CI job uses it is in [ci-integration.md](ci-integration.md).

## Reproducing a run

A run is reproducible from: the contract, the network, the observation boundary (the
ledger), the specification version, the profile, the bounds, and the engine version. Every
document the engine writes records the ones that apply, which is what lets two runs be
compared and a difference be believed.
