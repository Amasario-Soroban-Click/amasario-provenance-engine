# Example: a contract on testnet

The same inspection as `../local-contract`, against testnet, where the network is real
and the answer is not yours to control.

## The command

```console
cargo run -q -p amasario-cli -- inspect \
  --contract CDKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJJM5 \
  --network testnet
```

No `--rpc` is needed: testnet is a network the engine ships constants for, including its
passphrase, so it can check that the endpoint really serves testnet. Overriding `--rpc`
points the same inspection at your own node; overriding `--horizon` additionally enables
deployment resolution, which is what turns "this contract hosts this WASM" into "this
contract was deployed by this transaction".

The contract id above is the corpus's synthetic identity. It is a well-formed Soroban
address and it is not a deployed contract, so on a live network the honest result is
`CONTRACT` (exit `66`) rather than an analysis. Substitute the address you are actually
analysing; the engine does not care whose it is.

## What to look at

`--network` is not a label here, it is a claim the engine verifies. A testnet address
served by a mainnet endpoint is refused, because analysing the wrong chain produces a
confident answer about the wrong thing.

The observation boundary in the output is testnet's latest ledger at the time of the
run. Two runs a minute apart therefore have different boundaries, and a result is only
comparable to another result with the same boundary - which is what
`../snapshot-diff` is for.

## Needs a network

This example requires testnet. The `command` script honours `AMASARIO_RPC`, so the check
substitutes an address that refuses connections and requires exit `69` (`NETWORK`):
proof that the documented flags parse, and that a transport failure is classified rather
than reported as an absence of data.
