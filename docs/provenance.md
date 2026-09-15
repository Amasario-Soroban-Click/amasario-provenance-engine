# Provenance

Provenance answers: **where did this deployed executable come from, and how well do we
know?**

```bash
amasario provenance --contract CAAAA...D2KM --network testnet
amasario provenance --contract CAAAA...D2KM --network testnet --horizon https://horizon-testnet.stellar.org
```

## The chain

```
SOURCE → REVISION → BUILD → ARTIFACT → WASM → DEPLOYMENT → CONTRACT
```

Each arrow is a link, and each link is a claim with its own evidence and its own
verification status. The engine reports the chain link by link rather than as one verdict,
because a chain is not verified or unverified — it is verified *that far*.

| Link | What it asserts | Established by |
| --- | --- | --- |
| `SOURCE_RESOLVED` | The repository and revision exist and were read. | A source record and the revision it names. |
| `SOURCE_TO_BUILD` | The build ran against that revision. | Build metadata naming its source revision. |
| `BUILD_TO_ARTIFACT` | The build produced that artifact. | The artifact digest the build recorded. |
| `ARTIFACT_TO_WASM` | The artifact is that module. | The module digest, recomputed over the bytes. |
| `WASM_TO_DEPLOYMENT` | The deployment recorded that module. | The deployment's recorded module digest. |
| `DEPLOYMENT_TO_CONTRACT` | The contract exists at that deployment. | The deployment transaction and its effect. |

## The mistake this model exists to prevent

> Source code identity and deployed WASM identity are not automatically equivalent.

A repository that claims a revision, a build that claims a source revision, and a
deployment that records a module digest are three separate statements from three separate
sources. Any two of them can disagree, and the disagreement is the interesting case.

So the engine compares, and when a recorded digest does not match a computed one the
result is **`CONFLICTING`**, never `VERIFIED`. `fixtures/provenance/mismatched-wasm.json`
is exactly that case: a chain whose deployment records a module that disagrees with the
built one. A tool that marked it verified because all six links were "present" would be
reporting a contradiction as a confirmation.

## Statuses

| Status | Meaning | How it reaches a report |
| --- | --- | --- |
| `VERIFIED` | The evidence is consistent with the claim. | The link was established and nothing contradicted it. |
| `PARTIALLY_VERIFIED` | Part of the claim holds. | Some components checked out; none was contradicted. |
| `UNVERIFIED` | Nothing supports the claim. | The input needed to check it was never obtained. |
| `CONFLICTING` | The evidence refutes the claim. | A digest disagreed, or two sources disagreed. |
| `UNKNOWN` | The question could not be asked. | The contract executes no module, so there is nothing to verify. |

`CONFLICTING` takes precedence over every other status, and it is independent of
confidence: a claim can be confidently contradicted, which is what a mismatched digest is.
`UNKNOWN` is not a failure. A Stellar Asset Contract has no module, so there is no
executable identity to check, and treating that as a failed verification would make every
asset contract fail a provenance gate — which teaches a reader to ignore the gate.

## Confidence is capped by the basis

A link's confidence records how strong the evidence is; its status records whether the
evidence supports the claim. Nothing in a provenance chain can reach `VERIFIED`
*confidence*, because that level is reserved for a dependency established by an observed
invocation or event — the strongest basis the vocabulary has — and no build, artifact or
deployment relationship can be observed that way. A chain whose links are all `VERIFIED`
in status therefore carries `HIGH_CONFIDENCE`, and the distinction is exactly the one the
specification draws between "the evidence establishes this" and "this evidence is as
strong as evidence gets".

The `provenance` integration suite asserts the cap on every committed fixture, so a link
claiming more than its basis permits fails the build.

## Fixtures

| Fixture | What it demonstrates |
| --- | --- |
| `verified.json` | Every link established, every link verified. |
| `partial.json` | Source and build established, the deployment not. |
| `unknown-source.json` | One link and no more, because the chain stops where the evidence does. |
| `mismatched-wasm.json` | A deployment whose module digest disagrees with the built one: `CONFLICTING`. |

## Attestations

An attestation is an external statement about the build, and the engine identifies one so
that evidence can cite it. `ATTESTED` is a basis in its own right, capped at
`HIGH_CONFIDENCE`, because an attestation is only as good as its issuer and the issuer is
not something the engine can verify. The engine does not treat an attestation as a
substitute for recomputing a digest.

## Reproducibility

The engine reports what it can check. A build that recorded its toolchain, its
configuration and its lockfile gives the engine more to compare; one that recorded nothing
gives it nothing to compare, and the link is `UNVERIFIED` with the reason stated. The
engine does not attempt to rebuild a contract, and does not claim that a matching digest
proves a given source produced it — only that the recorded digest agrees with the computed
one.

## What provenance is not

A fully verified provenance chain is a statement about the *consistency of evidence*. It is
not a statement about intent, quality or safety. A deliberately backdoored contract with a
complete, honest, verified provenance chain is entirely possible, and the engine would
report it as verified. Every report the engine writes carries that disclaimer. See
[security.md](security.md).
