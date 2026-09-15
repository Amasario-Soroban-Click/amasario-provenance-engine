# Example: a chain that cannot be resolved

The counterpart to `../verified-provenance`. Here nothing is claimed, so there is nothing
to verify, and the honest answer is `UNKNOWN` with the reason recorded - not a failure,
and not a fabricated provenance chain.

## The command

```console
cargo run -q -p amasario-cli -- provenance \
  --contract CDKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJJM5 \
  --network testnet \
  --format json \
  --pretty
```

No `--source-repo` and no `--revision`. The engine observes that the contract exists and
what module it hosts, and stops there. It does not guess a repository from the contract's
name, does not search for a commit that happens to produce a similar digest, and does not
report a partially-populated chain as though the missing links were merely unevaluated.

## What the result says

`UNVERIFIED`, not `UNKNOWN`: the engine asked the question and could not answer it,
which is different from never having asked. `UNKNOWN` is reserved for the case where not
even the question could be posed.

The report's `unknown` section is where the unanswered question is written down, with a
machine-readable reason. `fixtures/expected-reports/unknown-provenance.json` carries one
entry:

```json
{
  "question": "which source revision the deployed module was built from",
  "reason": "UNSUPPORTED",
  "detail": "no repository or revision is recorded for this contract"
}
```

That entry is the whole value of the example. An engine that reported an empty result
would be telling a reader that there is no provenance to find; this one tells them what
is missing, why, and what would have to be supplied to answer it.

## What would turn this into a contradiction

`CONFLICTING` is the third outcome, and it is reached by *claiming* something false
rather than by claiming nothing. Supply a `--revision` whose rebuild does not produce the
deployed module's digest and the status becomes `CONFLICTING`, which takes precedence
over every other status. An incorrect `VERIFIED` is the most damaging output this system
can produce, because it is the one that stops a reader looking further, so the model is
built so that contradiction cannot be rounded to success.

## Needs a network

This example requires testnet. `command` honours `AMASARIO_RPC` so the check can point it
at an address that refuses connections and require exit `69` (`NETWORK`).
