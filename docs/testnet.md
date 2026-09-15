# Working against a live network

This page is the practical path from a checkout to an analysis of a real contract.

## Before anything: install and build

```bash
scripts/install.sh     # installs the toolchain components and builds the workspace
scripts/test-testnet.sh # analyses the committed live target and asserts the result
```

`scripts/install.sh` builds the workspace and `scripts/install.ps1` does the same on
Windows. Neither installs anything outside this repository.

Or build directly:

```bash
cargo build --release -p amasario-cli
./target/release/amasario --help
```

## Testnet

Testnet has published endpoints, so no overrides are needed.

```bash
amasario inspect      --contract <CONTRACT_ID> --network testnet
amasario provenance   --contract <CONTRACT_ID> --network testnet
amasario dependencies --contract <CONTRACT_ID> --network testnet --depth 5
amasario graph        --contract <CONTRACT_ID> --network testnet --format dot -o graph.dot
amasario impact       --contract <CONTRACT_ID> --network testnet
amasario report       --contract <CONTRACT_ID> --network testnet --format markdown -o report.md
```

Add `--horizon` to enable deployment resolution, and `--scan-events` to look for
cross-contract calls:

```bash
amasario provenance --contract <CONTRACT_ID> --network testnet \
  --horizon https://horizon-testnet.stellar.org \
  --scan-events
```

Without `--horizon`, the deployment links of the provenance chain are `UNVERIFIED` and the
report says why. Without `--scan-events`, contract-to-contract dependencies are only
discoverable from transaction and operation history, which is thinner.

## The snapshot workflow

```bash
# Capture the state now.
amasario snapshot create --contract <CONTRACT_ID> --network testnet -o before.json

# ... a deployment, an upgrade, or simply time passing ...

amasario snapshot create --contract <CONTRACT_ID> --network testnet -o after.json
amasario diff --before before.json --after after.json --format markdown
```

A diff of two captures of the *same* contract at different ledgers is the finding. A diff
of two different contracts is reported as incomparable with the reason rather than compared
anyway — see [snapshots.md](snapshots.md).

## The smoke test

```bash
scripts/test-testnet.sh                    # the committed target, on testnet
scripts/test-testnet.sh <CONTRACT_ID> testnet
```

With no arguments the script analyses the contract named in `scripts/live-target.env` — a
Soroban market that both *is* called and *makes* calls to another contract in ordinary
activity, which is the smallest real case that exercises observation, cross-contract call
recovery, dependency classification, evidence, confidence and impact end to end. The
scheduled workflow reads the same file, so there is one place that says what is tested.

The script asserts, rather than only running. It checks the documents each command
produces for the properties that must hold for any successful analysis of a reachable
contract — every dependency edge carries evidence, a basis, a confidence level and an
observation boundary, and each status is drawn from the specification's taxonomy — and for
the one property that must hold for the committed target: that a contract which calls
another contract yields at least one verified `INVOCATES` edge.

What it deliberately does **not** assert is anything a live network changes under it: a
ledger number, a transaction hash, an event count, or the identity of any dependency.
Those belong to the fixture corpus, where they are deterministic — see
[ci-integration.md](ci-integration.md). A live assertion that pinned them would fail for
reasons unrelated to the change under review, which is how a live test becomes one people
learn to ignore.

The one assertion that depends on the world is the verified edge. If the target stops
being called the script fails with a message saying so, and the fix is to widen
`AMASARIO_LOOKBACK` or name a busier contract in `scripts/live-target.env`. That failure
is the test doing its job.

It performs only read-only analysis. Nothing it runs has a mutating mode to submit a
transaction with.

Environment:

| Variable | Meaning |
| --- | --- |
| `AMASARIO_BIN` | The binary to run. Defaults to the release build, then the debug build, then `PATH`. |
| `AMASARIO_RPC` | The RPC endpoint, which `mainnet` and `futurenet` need. |
| `AMASARIO_ARTIFACTS` | Where the outputs are written. Unset, they go to a temporary directory that is removed on exit. |
| `AMASARIO_LOOKBACK` | How many ledgers of history the event scan covers. Defaults to the CLI's 256, about a day. |

## Local and private chains

A local standalone network, a private deployment and a partner environment all identify
themselves by passphrase. None of them is one of the three networks the engine ships
constants for, so the passphrase is what lets the engine check that the endpoint really
serves that chain.

```bash
amasario inspect \
  --contract <CONTRACT_ID> \
  --rpc http://localhost:8000/soroban/rpc \
  --horizon http://localhost:8000 \
  --passphrase 'Standalone Network ; February 2017' \
  --network-name local
```

The passphrase value above is the one Stellar's own `quickstart` standalone network uses;
it is documented here because it is the passphrase a developer running a local chain will
need, and it is a published constant rather than a secret. If you run your own chain, use
its passphrase.

The engine compares the endpoint's reported chain identity against what it was told. A
mismatch fails the run before anything is analysed. That check is the reason a custom
network requires a passphrase at all: without one it would have to be skipped, and a
skipped check is how a testnet analysis becomes a mainnet analysis.

## Futurenet and mainnet

```bash
amasario inspect --contract <CONTRACT_ID> --network futurenet \
  --rpc <RPC_URL> --horizon <HORIZON_URL>

amasario inspect --contract <CONTRACT_ID> --network mainnet \
  --rpc <RPC_URL> --horizon <HORIZON_URL>
```

Neither network's endpoint is stable enough for the engine to publish a constant for, so
both require `--rpc` and `--horizon`. Supply endpoints you trust. Read
[security.md](security.md) before pointing the engine at mainnet, and remember that a
snapshot of a mainnet contract is data about someone else's project.

## Bounds on a live network

Live analysis is where unbounded work is expensive rather than merely incorrect.

| Bounds | Start with | Notes |
| --- | --- | --- |
| `--depth 5` | A contract whose dependencies matter. | Each hop is a network read. |
| `--max-nodes 10000` | The default. | Lower it when the graph is large and you only need the near field. |
| `--max-transactions 16` | Raise it when a contract is busy. | History is paged, so this is the number of records, not requests. |
| `--scan-events` | Only when you need cross-call history. | The most expensive read the engine makes. |

A traversal that hits a bound says so in its output. When you see `truncated`, raising the
bound is a decision with a cost, and leaving it is a decision with a caveat: the affected
set reported is smaller than exists. See [impact-analysis.md](impact-analysis.md).

## Troubleshooting a live run

See [troubleshooting.md](troubleshooting.md) for the errors this produces and what each of
them means, including the ones that look like an empty result and are not.
