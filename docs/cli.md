# Command line

The binary is `amasario`. It has eleven commands, of which `snapshot` has two
subcommands because capturing and reading a snapshot are different jobs with different
inputs.

```
amasario <COMMAND>

Commands:
  inspect       Inspect a contract: identity, executable, interface and observed activity
  discover      List the contracts observed in the target's company, without deciding dependencies
  provenance    Establish where the deployed executable came from, and how well
  dependencies  Resolve what the contract depends on, with a basis and evidence per edge
  graph         Build the dependency graph and export it
  impact        Analyse what a change to an entity could reach
  snapshot      Capture a normalised snapshot, or read one back
  diff          Compare two snapshots
  verify        Check whether the evidence supports the executable's identity, as a gate
  report        Generate the specification's report in any of its four renderings
  export        Export a recorded analysis as JSON, YAML, GraphML or DOT
```

Every command that observes a network takes the same options, so the network and the
observation boundary are chosen the same way everywhere.

## Choosing what to analyse

| Option | Meaning |
| --- | --- |
| `--contract <CONTRACT>` | The contract address to analyse. Required by every observing command. |
| `--network <NETWORK>` | `futurenet`, `testnet` or `mainnet`. Defaults to `testnet`. |
| `--rpc <URL>` | Override the RPC endpoint. Required for a network the engine has no published endpoint for. |
| `--horizon <URL>` | Override the Horizon endpoint, which is what enables deployment resolution. |
| `--passphrase <PASSPHRASE>` | The passphrase of a network the engine has no published identity for. |
| `--network-name <NAME>` | The name to record for a custom network. Defaults to `custom`. |

### Which networks work out of the box

| Network | RPC | Horizon | Notes |
| --- | --- | --- | --- |
| `testnet` | published | published | The default, and what [testnet.md](testnet.md) uses. |
| `futurenet` | requires `--rpc` | requires `--horizon` | A moving network; its endpoint is not stable enough to publish. |
| `mainnet` | requires `--rpc` | requires `--horizon` | Supply an endpoint you trust, and read [security.md](security.md) first. |
| a local or private chain | requires `--rpc` | optional | Requires `--passphrase` too. See below. |

A local standalone network, a private deployment and a partner environment all identify
themselves by passphrase. None of them is one of the three networks the engine ships
constants for, so `--passphrase` is what lets the engine check that the endpoint really
serves that chain. Without it the check would have to be skipped, and a skipped check is
how a testnet analysis becomes a mainnet analysis.

```bash
amasario inspect \
  --contract CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM \
  --rpc http://localhost:8000/soroban/rpc \
  --horizon http://localhost:8000 \
  --passphrase 'Standalone Network ; February 2017' \
  --network-name local
```

## Bounds

| Option | Meaning | Default |
| --- | --- | --- |
| `--depth <N>` | How many edges traversal may cross before it must stop and say so. | 6 |
| `--max-nodes <N>` | The most entities a traversal may accumulate. | 10000 |
| `--max-transactions <N>` | The most transactions to read for invocation evidence. | 16 |
| `--scan-events` | Scan the contract's events, which is what finds cross-call invocations. | off |
| `--lookback <LEDGERS>` | How far back an event scan reaches, ending at the boundary. | 256 |
| `--from-ledger <LEDGER>` | Scan forward from this ledger instead of over a recent window. | unset |
| `--max-event-pages <N>` | How many event pages the scan may read before it stops and says so. | 10 |

`MAX_PERMITTED_DEPTH` is 32. A larger `--depth` is clamped rather than refused: the
analysis still runs, and the bound it actually used is the one recorded in the result. A
bound of zero is refused, because a traversal that can visit nothing produces an empty
answer that reads like a complete one.

`--scan-events` is off by default because it is the most expensive read the engine makes
and because a contract with no events has no cross-call history to find. Turning it on is
what makes a contract-to-contract dependency discoverable rather than only a
contract-to-WASM one.

### The event horizon, which is the bound that decides whether the answer is useful

`getEvents` is a ledger-ordered feed that paginates by **event count**, not by ledger: one
request returns as many events as its limit allows from the ledger it starts at, onward.
That has a consequence a caller has to know about, because it decides what a scan can
possibly find.

A scan starting at the oldest ledger a node retains spends its page budget on the *oldest*
events in the retention window - on a public endpoint, the oldest few minutes of seven
days - and never reaches anything that happened since. `--lookback` therefore anchors the
window at the **tip**, not at the retention floor, and the default is 256 ledgers, about
twenty minutes. The window has to be small enough that the page budget can cover it, which
is why the default is minutes rather than days: under-covering is reported, over-covering
cannot happen.

```console
# The default: the most recent ~20 minutes of activity.
amasario dependencies --contract CABC... --network testnet --scan-events

# A longer horizon. Expect MAX_NODES_REACHED on a busy contract, and read it as
# "the scan reached back this far", not as "the contract did nothing older".
amasario dependencies --contract CABC... --network testnet --scan-events \
  --lookback 17280 --max-event-pages 50

# A deliberate historical window, walked forward from a named ledger.
amasario dependencies --contract CABC... --network testnet --scan-events \
  --from-ledger 4000000
```

A `RATE_LIMITED` or `MAX_NODES_REACHED` truncation therefore does not mean the scan
failed; it means the scan stopped and told you where. Only `RATE_LIMITED` and `CANCELLED`
are transient, so only those merit a re-run at the same settings - a wider `--lookback`
reaches further back, and re-running the same command reaches the same place.

## Output

| Option | Meaning |
| --- | --- |
| `--format <FORMAT>` | `text` (default), `json`, `markdown`, `dot`, `junit`. |
| `-o, --output <PATH>` | Write to a path instead of stdout. |
| `--pretty` | Indent JSON and YAML output for a person to read. |

| Format | For |
| --- | --- |
| `text` | A person reading a terminal. |
| `json` | A pipeline. This is the specification's document, canonically written. |
| `markdown` | A person reading a file, or a pull-request comment. |
| `dot` | A graph renderer. Available where a graph is in hand. |
| `junit` | A test reporter, so a CI job can gate on a provenance check. |

`amasario export --format` additionally accepts `yaml` and `graphml`. See
[output-formats.md](output-formats.md) for the exact shape of each.

## `amasario inspect`

Contract identity, executable, module digest, interface where one could be decoded, and
observed activity. What was observed is separated from what was inferred and from what
could not be obtained.

```bash
amasario inspect --contract CAAAA...D2KM --network testnet
amasario inspect --contract CAAAA...D2KM --network testnet --format json --pretty
```

## `amasario discover`

Lists the contracts observed in the target's company **without deciding dependencies**. It
is deliberately separate from `dependencies`: naming what was seen is an observation, and
calling two contracts dependent is a claim that needs a basis and evidence.

```bash
amasario discover --contract CAAAA...D2KM --network testnet --depth 3 --scan-events
```

## `amasario provenance`

Assembles source → revision → build → artifact → WASM → deployment and reports how far the
evidence reaches and what each link is worth.

```bash
amasario provenance --contract CAAAA...D2KM --network testnet
```

## `amasario dependencies`

The dependency set, with a basis, a confidence and at least one citation per edge, and a
published entry for every candidate a rule refused.

```bash
amasario dependencies --contract CAAAA...D2KM --network testnet --depth 5
amasario dependencies --contract CAAAA...D2KM --network testnet --format json --pretty
```

## `amasario graph`

Builds the typed graph from the resolved set and renders it.

```bash
amasario graph --contract CAAAA...D2KM --network testnet --format dot -o graph.dot
amasario graph --contract CAAAA...D2KM --network testnet --format json --pretty
```

## `amasario impact`

Propagates a declared change against the direction each relationship declares, within the
bounds, and reports the findings and whether the analysis was bounded.

```bash
amasario impact --contract CAAAA...D2KM --network testnet
amasario impact --contract CAAAA...D2KM --network testnet --depth 2
```

## `amasario snapshot`

```bash
amasario snapshot create --contract CAAAA...D2KM --network testnet -o snapshot-a.json
amasario snapshot create --contract CAAAA...D2KM --network testnet --format json --pretty
amasario snapshot show --input snapshot-a.json --format json --pretty
```

`snapshot create` runs the pipeline and writes the normalised capture. `snapshot show`
reads a stored snapshot back and reports what it holds — which is also the check that a
snapshot written by an older engine is still readable.

## `amasario diff`

```bash
amasario diff --before snapshot-a.json --after snapshot-b.json
amasario diff --before snapshot-a.json --after snapshot-b.json --format markdown
```

An incomparable pair — two different contracts, or one contract on two networks — is
reported as incomparable with the reason, not compared anyway.

## `amasario verify`

The gate. It asks whether the evidence supports the executable's identity and exits
non-zero when it does not, which is the case a CI job is meant to stop on.

```bash
amasario verify --contract CAAAA...D2KM --network testnet
amasario verify --contract CAAAA...D2KM --network testnet \
  --claimed-digest 9f2c...a41b --require verified
```

| Option | Meaning |
| --- | --- |
| `--claimed-digest <HEX>` | A claimed SHA-256 executable digest to compare against what the network reports. |
| `--require <STATUS>` | Require at least `verified`, `partially` or `unverified`. |

The outcome is the specification's status, and the five statuses are not degrees of one
scale. `CONFLICTING` fails the gate without being asked, because a claim the evidence
refutes is not a weak verification — it is a refutation. `UNKNOWN` passes unless
`--require` names a level it does not reach: a Stellar Asset Contract has no module, so
there is nothing to verify, and failing every asset contract would train a reader to
ignore the gate. See [verification.md](verification.md).

## `amasario report`

```bash
amasario report --contract CAAAA...D2KM --network testnet --format markdown -o report.md
amasario report --contract CAAAA...D2KM --network testnet --format junit -o junit.xml
amasario report --contract CAAAA...D2KM --network testnet --format json --pretty
```

## `amasario export`

```bash
amasario export --input snapshot-a.json --format yaml -o analysis.yaml
amasario export --input snapshot-a.json --format graphml -o graph.graphml
amasario export --input snapshot-a.json --format dot -o graph.dot
```

## Exit codes

`amasario` exits `0` when the command produced its result. Otherwise the code names what
went wrong, so a script can branch without parsing stderr.

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | `verify` refused: the outcome is contradicted, or short of `--require`. |
| `2` | A usage or configuration error: a malformed argument, a zero bound, a mismatched passphrase. |
| `65` | `EX_DATAERR`: a provenance, dependency, graph, impact or specification-compatibility failure. |
| `66` | `EX_NOINPUT`: the contract, the snapshot or the file named does not exist or could not be read. |
| `69` | `EX_UNAVAILABLE`: the network did not answer. |
| `70` | `EX_SOFTWARE`: a report or export could not be produced. |
| `74` | `EX_IOERR`: writing the output failed. |

The numbers follow the `sysexits` convention where one applies. They are the tool's, not
the specification's, which describes error categories rather than process codes.

## Credentials

The engine takes no credential. There is no API key, no mnemonic and no private key in
its interface, which is why there is nothing here about passing one safely. A read-only
RPC endpoint needs no secret. See [security.md](security.md) for what the engine does and
must not be trusted to do.
