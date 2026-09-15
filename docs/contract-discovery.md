# Contract discovery

Discovery answers a narrower question than dependency analysis: **what has been observed
about this contract**, without deciding what it depends on. The two are separate commands
for a reason. Naming what an endpoint returned is an observation; calling two contracts
dependent is a claim that needs a basis and evidence. Collapsing them is how a tool ends
up asserting a dependency because two contracts appeared in one response.

```bash
amasario inspect --contract CAAAA...D2KM --network testnet
amasario discover --contract CAAAA...D2KM --network testnet --depth 3 --scan-events
```

## What is read

| Observation | Source | Confidence it can support |
| --- | --- | --- |
| Contract identity: address, network, ledger | RPC ledger entry | Observed |
| Executable: `WASM(hash)` or `STELLAR_ASSET` | RPC ledger entry | Observed |
| Module bytes | RPC `getLedgerEntries` | Observed |
| Module digest, recomputed | The bytes themselves | Observed |
| Interface, where a contract spec section exists | The module's `contractspecv0` section | Observed |
| Invocations | Horizon operations and transactions, and contract events | Observed |
| Deployment | The transaction that created or upgraded the instance | Observed |

## Observed, inferred, unknown

Every field the engine reports is one of three things, and the engine does not blur them.

* **Observed** — the endpoint returned it at the boundary. It carries a citation.
* **Inferred** — it follows from observations by a stated basis. It carries the basis and
  the citations it was inferred from.
* **Unknown** — it could not be obtained, and the engine says so rather than omitting it.
  An omitted field reads as "not applicable"; an unknown field records the question.

The distinction is visible in the output. A contract whose interface could not be decoded
reports that fact; it does not report an empty interface, because an empty interface means
"this contract takes no arguments and exposes no functions", which is a different claim.

## The interface is decoded or it is not reported

A Soroban contract's interface lives in a custom section named `contractspecv0`. Many real
modules do not have one, and a module that exports a function is not a module that
declares a contract interface. The engine therefore reports an interface only when it
decoded one from that section, and reports the absence explicitly otherwise.

`fixtures/wasm/` holds the cases the decoder is held to, and the `contracts` integration
suite asserts each one:

| Fixture | What it is | Expected |
| --- | --- | --- |
| `empty.json` | A valid module with no sections at all. | Decodes. No interface. |
| `custom-section.json` | A valid module whose only section is a custom one. | Decodes. The custom section is skipped. |
| `export-without-spec.json` | A valid module exporting `greet`, with no contract spec section. | Decodes. No interface, and no invented one. |
| `truncated-header.json` | The magic number with no version word. | Refused by the decoder. |
| `wrong-version.json` | The right magic and the wrong version word. | Refused by the decoder. |

`looks_like_module` is the cheap pre-check: it reads the magic number only, and is
documented as such rather than as "is a module". The last two fixtures are why the
distinction is stated: both pass the pre-check and both must be refused by the decoder. A
pre-check that answered the full question would be as expensive as the decoder.

## Identity is not an address

A contract address alone does not establish provenance. The engine's identity model
carries the address, the network, the module digest, the ledger the observation was made
at, the first and last ledgers it was seen at, and the deployment transaction where one
could be resolved.

The pair of fixtures `wasm-contract.json` and `wasm-contract-after-upgrade.json` is the
same address observed at two ledgers with two different modules. That is what an upgrade
looks like, and it is why "this contract" and "this contract running this module now" are
separate statements. A tool that keyed only on the address would report an upgrade as no
change at all.

## Deployment resolution, and why it needs Horizon

The RPC endpoint serves the current ledger state. It does not serve history, so it cannot
say which transaction deployed a contract. That is what `--horizon` is for.

```bash
amasario inspect --contract CAAAA...D2KM --network testnet \
  --horizon https://horizon-testnet.stellar.org
```

Without `--horizon`, the deployment link of the provenance chain is `UNVERIFIED`, and the
report says why. It is not reported as "no deployment", which would be a claim the engine
cannot support without looking.

## Events, and what they add

`--scan-events` reads the contract's events and treats a recorded cross-contract call as
evidence of an invocation. It is off by default because it is the most expensive read the
engine makes, and because a contract with no events has no cross-call history to find.

With events on, a contract-to-contract dependency becomes discoverable rather than only a
contract-to-WASM one. The evidence class differs — an event is not a transaction — and the
engine keeps them apart, so a dependency found only in an event and one found in a
successful transaction are distinguishable in the output.

## The boundary

Every observation is stamped with the network, the ledger and the time. Two analyses are
comparable only when their boundaries are compatible; two snapshots of one contract at
different networks are reported as incomparable rather than diffed. See
[snapshots.md](snapshots.md).

## Non-existent contracts

A contract that does not exist is reported as an absence: the endpoint answered, and the
answer was that there is no such entry. That is distinct from an unreachable endpoint
(`EX_UNAVAILABLE`, `69`) and from a malformed response. The command exits non-zero, and
the message says which of the three happened.

## What discovery does not do

It does not enumerate the network, it does not scan arbitrary addresses, and it does not
decide what anything depends on. It observes one contract, its executable, its
interactions and its deployment, within a declared boundary.
