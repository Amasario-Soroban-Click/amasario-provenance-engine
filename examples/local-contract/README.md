# Example: a contract on a local standalone chain

This is the first thing anyone runs, because it is the only environment where the answer
is fully under your control: you deployed the contract, so you know what the engine
should observe.

## The command

```console
cargo run -q -p amasario-cli -- inspect \
  --contract CDKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJJM5 \
  --passphrase "Standalone Network ; February 2017" \
  --network-name local \
  --rpc http://localhost:8000/soroban/rpc
```

A local chain has no published endpoint and no published identity, so both are supplied:
`--passphrase` is what tells the engine *which* chain the endpoint serves, and it is
checked rather than trusted. `--network-name` labels the result, and the exit code is
`2` if `--rpc` is omitted, because a custom network has no default to fall back to.

`--passphrase "Standalone Network ; February 2017"` is the passphrase `stellar
contract init` and the Soroban quickstart's Docker image use for a standalone network.
If your node was started with a different passphrase, this example fails with a network
identity mismatch rather than silently analysing the wrong chain - which is the point of
supplying it.

## What to look at

The output names the contract's identity first and its executable hash separately, and
that separation is deliberate: the address survives an upgrade and the hash does not. An
`inspect` that reported only the address would be reporting a name, not an artifact.

Then it reports what it observed: the ledger it read, the interface it could obtain, and
any invocations it saw. Everything it could not obtain is reported as absent rather than
inferred - a contract whose interface cannot be decoded is a contract whose interface
could not be decoded, not one with no interface.

## Needs a network

This example requires a running local node at the address passed to `--rpc`. The
`command` script honours `AMASARIO_RPC` so that a check can substitute an address that
refuses connections, and the run then exits `69` (`NETWORK`) - a classified transport
failure, not an empty result.
