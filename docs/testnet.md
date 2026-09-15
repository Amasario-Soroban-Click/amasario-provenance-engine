# Working against a live network

This page is the practical path from a checkout to an analysis of a real contract.

## Before anything: install and build

```bash
scripts/install.sh          # installs the toolchain components and builds
scripts/test-testnet.sh <CONTRACT_ID> testnet
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
scripts/test-testnet.sh <CONTRACT_ID> [network]
```

This is a smoke test, not a test suite. It proves the CLI reaches a real endpoint, that the
network identity check passes against the chain it was pointed at, and that each command
renders. Its outputs are **not asserted**: a live network is not deterministic, and
asserting them would either fail constantly or assert nothing. Deterministic assertions
belong to the fixture corpus — see [ci-integration.md](ci-integration.md).

It performs only read-only analysis. Nothing it runs has a mutating mode to submit a
transaction with.

Environment:

| Variable | Meaning |
| --- | --- |
| `AMASARIO_BIN` | The binary to run. Defaults to the workspace build, then `PATH`. |
| `AMASARIO_RPC` | The RPC endpoint, which `mainnet` and `futurenet` need. |

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
