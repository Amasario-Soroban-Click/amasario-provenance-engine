# Verification

Verification answers one question: **does the evidence support the claim?**

It is not a score. The five statuses are not degrees of one scale, and two of them mean
opposite things.

| Status | Meaning |
| --- | --- |
| `VERIFIED` | The evidence is consistent with the claim. |
| `PARTIALLY_VERIFIED` | Part of the claim holds, and nothing was contradicted. |
| `UNVERIFIED` | Nothing supports the claim, and nothing contradicted it either. |
| `CONFLICTING` | The evidence refutes the claim, or two sources disagree. |
| `UNKNOWN` | The question could not be asked. |

## Status and confidence are independent

A status says whether the evidence **supports** a claim. A confidence says how **strong**
the evidence is. They move independently:

* a claim can be `VERIFIED` at `HIGH_CONFIDENCE` — the evidence establishes it, and the
  basis is a digest rather than an observation, which caps the level;
* a claim can be `CONFLICTING` at `HIGH_CONFIDENCE` — two digests were compared and they
  disagreed, which is a strong finding that the claim is false;
* a claim can be `UNVERIFIED` at `UNKNOWN` — nothing was obtained to check against.

Collapsing the two would make `CONFLICTING` read as "less verified than verified", which
is exactly backwards. A contradiction is not a weak verification; it is a refutation.

## `CONFLICTING` is the status that matters

The specification introduces `CONFLICTING` for one reason: a claimed source revision that
cannot be matched to the deployed WASM must be representable as a contradiction rather
than silently marked verified.

The engine's cases:

| Situation | Status |
| --- | --- |
| The retrieved module's bytes hash to the digest the network records. | `VERIFIED` |
| The retrieved module's bytes do not hash to the recorded digest. | `CONFLICTING` |
| The digest was recorded but the module was never retrieved. | `UNVERIFIED` |
| A claimed digest disagrees with the recorded one, even though the module verified. | `CONFLICTING` |
| The contract executes no module at all. | `UNKNOWN` |

A claimed digest is a second claim. When one is supplied, it is compared against what the
network reports, and a mismatch is a contradiction regardless of whether the module itself
verified — because two claims about one digest disagreeing is a conflict, and resolving it
by preferring one would be the engine choosing a truth it cannot know.

## The gate

```bash
amasario verify --contract CAAAA...D2KM --network testnet
amasario verify --contract CAAAA...D2KM --network testnet --require verified
amasario verify --contract CAAAA...D2KM --network testnet \
  --claimed-digest 9f2c...a41b --require partially
```

| `--require` | Passes |
| --- | --- |
| *(absent)* | Everything except `CONFLICTING`. |
| `unverified` | `VERIFIED`, `PARTIALLY_VERIFIED`, `UNVERIFIED`. |
| `partially` | `VERIFIED`, `PARTIALLY_VERIFIED`. |
| `verified` | `VERIFIED` only. |

`CONFLICTING` fails every requirement, including the absent one, because it is not below
`VERIFIED` — it is a different thing. `UNKNOWN` passes unless `--require` names a level it
does not reach.

Exit codes: `1` for a refused gate, `2` for a usage error. The full table is in
[cli.md](cli.md).

## Why a status is not a boolean

A CI job that asked "is it verified, yes or no" would have to answer `UNVERIFIED` for two
very different situations: the engine looked and found nothing supporting the claim, or
the engine could not look at all. The first is a finding; the second is an incomplete
analysis. The gate can be configured to accept one and refuse the other, and the report
says which happened.

## What the engine refuses to claim

The engine does not report, and the specification forbids it from reporting:

* that a contract is **secure**;
* that a contract is **safe**;
* that a contract is **malicious**;
* that a contract is **vulnerable**;
* that a contract is **free of vulnerabilities**.

`VERIFIED` states that the evidence is consistent with the claim it is attached to. It
states nothing about the trustworthiness of the entity the claim concerns. A fully
verified provenance chain can describe a deliberately backdoored contract. Every report
the engine writes carries that disclaimer, in the report document itself rather than only
in the documentation — see [security.md](security.md) and
[output-formats.md](output-formats.md).

## Where verification is enforced

| Layer | What it verifies |
| --- | --- |
| `amasario-network` | That the endpoint serves the chain the caller named, by passphrase. |
| `amasario-contract` | That the retrieved module's bytes hash to the digest the network records. |
| `amasario-provenance` | Each chain link, against the evidence cited for it. |
| `amasario-evidence` | That a record satisfies its own class before it supports a claim. |
| `amasario-dependency` | That a candidate's basis can establish the claim it makes. |
| `amasario-snapshot` | That a snapshot's content digest agrees with its contents. |

The `verification` integration suite is the cross-cutting pass: it reads every committed
fixture, checks that each parses as the document the specification defines, and asserts
that a document the engine wrote is a document the engine can read back.

## Reproducing a verification

A verification is reproducible from the contract, the network, the ledger boundary, the
profile, the engine version and the specification version. A `CONFLICTING` result is
therefore something two parties can independently reproduce rather than something they
have to take on trust — which is the only basis on which a refutation is worth acting on.
