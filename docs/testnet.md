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

## The first-party reference contract on Testnet

`reference-contract/` is deployed to Testnet, and it is the only thing this project
deploys. Two contracts, both built from this repository's source, both committed as the
fixtures under `fixtures/reference/`:

| | Contract | Committed module digest |
| --- | --- | --- |
| callee — `Ledger` | `CBMPDHYWBGBJ4JAUKNLE6OTC4LQTLV3XFVMAN72MCFSMN2EOJPYEXK6N` | `347286105091f6d2d5db47ef5ae4442550a4d18f3a0f11284b59c22211031ac7` |
| caller — `Observer` | `CBNCEDVA7SQ2NSNGG7RGQOK4VESBN2YSCLJ6DSHRL6QH72VPR5MYIVCA` | `b361a71a39976897beb916fc85faa5670617ff95d68a5cecb00e7deabcedb7b7` |

The deployer is `GATPEYEMBUZB655G323G5AIR77JSYFVK354EI6V7RVWWXGYMTHEIYTHZ`, and the four
transactions below are on Testnet rather than described from a local run. Each was read
back from the network by `getTransaction`, and every one reported `SUCCESS`:

| Transaction | Ledger | What it did |
| --- | --- | --- |
| [`82e7727b…`](https://stellar.expert/explorer/testnet/tx/82e7727b4b10710fc67515fab61449e4790e969aaac631ccdcf08543df9a9a73) | 4710122 | deployed the callee |
| [`e8331640…`](https://stellar.expert/explorer/testnet/tx/e8331640d0fa6a6a92a4d72bb1206eaafd74a3d92acf0efd0da6a90f44d3a847) | 4710123 | deployed the caller |
| [`9f80b7dc…`](https://stellar.expert/explorer/testnet/tx/9f80b7dc5f14616a095448b464985604be5f61c1d8600c42f4710ce2642b6fd3) | 4710124 | the read-only cross-contract call, `caller -> callee` |
| [`a9445fb5…`](https://stellar.expert/explorer/testnet/tx/a9445fb5f4b3896d673e5089b19841469dea8683806de1cc32fce914e2be5268) | 4710125 | the authenticated call, which reached the callee's `require_auth` one frame down and emitted its `recorded` event |

### Reproducing it

The commands are committed, because a deployment that only ever happened on someone's
machine is a deployment nobody can check:

```bash
scripts/build-reference-contract.sh          # stage the modules the fixtures hold
scripts/deploy-reference-contract.sh --invoke
```

The script deploys the **fixtures**, not `target/`, and then checks the result with this
repository's own `amasario inspect`: it compares the wasm hash the chain reports against
the fixture's sha256, and stops if they differ. So a deployment it reports as successful is
one where the bytes on the chain and the bytes in the repository are the same bytes. Run
without `--invoke` it deploys and verifies only, and sends no transaction.

An identity is needed and the script does not create one, because a funded account is a
decision about whose account it is:

```bash
stellar keys generate <name> --network testnet --fund
scripts/deploy-reference-contract.sh --identity <name> --invoke
```

No secret is in this repository, and the engine holds no key: the script names an identity
and the `stellar` CLI signs with the one in its own keystore.

### What the deployment is evidence for

Both deployed modules hash to the committed fixtures, so the divergence between “the module
this repository builds” and “the module that is running” is zero and is checked on the
chain rather than asserted in prose.

The cross-contract edge is then observable, which is the point of deploying both halves:

```console
$ amasario discover --contract CBMPDHYWBGBJ4JAUKNLE6OTC4LQTLV3XFVMAN72MCFSMN2EOJPYEXK6N \
    --network testnet --scan-events
discovery   CBMPDHYWBGBJ4JAUKNLE6OTC4LQTLV3XFVMAN72MCFSMN2EOJPYEXK6N
observed    1 transaction(s) read, 1 invocation(s) seen

CBNCEDVA7SQ2NSNGG7RGQOK4VESBN2YSCLJ6DSHRL6QH72VPR5MYIVCA - entered the target
  transactions: a9445fb5f4b3896d673e5089b19841469dea8683806de1cc32fce914e2be5268
```

**The subject matters, and the asymmetry is worth understanding rather than working
around.** The engine's evidence for a cross-contract relationship is an *event*: it reads
the events a contract emitted, then the transactions those events appeared in, and derives
from them what that contract called and what called it. Here only the callee publishes an
event, so asking the *caller* about its dependencies establishes nothing:

```console
$ amasario dependencies --contract CBNCEDVA7SQ2NSNGG7RGQOK4VESBN2YSCLJ6DSHRL6QH72VPR5MYIVCA \
    --network testnet --scan-events
evidence     0 invocation(s) observed from 0 transaction(s) read

no dependency was established. This is the absence of evidence, not evidence of the
absence of a dependency: a call that left no trace on the chain cannot be observed.
```

That is the tool working as documented, not a gap in it, and the caller contract is
written the way it is on purpose. A reader who expects `dependencies` on the caller to
report the callee has a reasonable expectation and a wrong model, so it is stated here: ask
the contract that *emitted* something. This is also why a fresh deployment with no
invocations shows nothing — the edge is history, and the history has to be made.

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
| `AMASARIO_LOOKBACK` | How many ledgers of history the event scan covers. Defaults to the CLI's 256, which at Stellar's roughly five-second close time is about twenty minutes. |

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
