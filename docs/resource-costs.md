# Resource costs

What the reference contract costs to call, measured on Testnet rather than estimated.

## Soroban does not have gas

The mental model to discard first. There is no per-operation gas price and no gas limit,
so a document promising a "gas benchmark" for a Soroban contract is promising the wrong
number. Soroban meters a **resource budget** instead, and the network charges a fee for
the resources an invocation actually consumed:

| Metered | What it covers |
| --- | --- |
| CPU instructions | Every WASM instruction the host executed, including in contracts the call reached |
| Memory | Peak linear memory the invocation used |
| Ledger reads and writes | Entries the transaction's footprint touched, priced by their size |
| Transaction size | The bytes of the envelope |
| Events | The topics and data of everything published |

The fee is charged in two parts and this is the part worth understanding before reading
the table. The **non-refundable** part pays for work that cannot be undone once done -
instructions, ledger I/O, size - and is billed in full. The **refundable** part is
charged up front against a budget and then refunded to the extent it was not used, so a
transaction that reserves generously and consumes little is refunded the difference.
That is why the fee the simulator *proposes* and the fee the network *charges* differ by
a factor of two or more, and why quoting the proposed figure as "the cost" would
overstate every number on this page.

## Measured

Both halves of the reference contract, on Testnet, each entrypoint sent as a real
transaction so the figure is a receipt rather than a simulation. Figures are in stroops;
1 XLM is 10,000,000 stroops.

| Entrypoint | Fee proposed | Fee charged | Non-refundable | Refunded | Charged in XLM |
| --- | --- | --- | --- | --- | --- |
| `callee.sequence` — read-only | 13,457 | 3,771 | 3,631 | 9,686 | 0.0003771 |
| `callee.record` — auth, storage write, event | 17,630 | 7,834 | 6,952 | 9,796 | 0.0007834 |
| `caller.observe` — cross-contract read | 14,199 | 4,514 | 4,374 | 9,685 | 0.0004514 |
| `caller.record_via` — cross-contract write, nested auth | 18,397 | 8,599 | 7,717 | 9,798 | 0.0008599 |

Reproduce with:

```bash
scripts/benchmark-contracts.sh --identity <funded-testnet-identity>
```

The script reads the contract ids out of [testnet.md](testnet.md) rather than keeping a
second copy, sends each call, and parses the fee breakdown the Stellar CLI prints under
`--cost`. It checks its own arithmetic: the reported total must equal the inclusion fee
plus both resource components, so a figure read from the wrong row fails the run instead
of producing a table of plausible numbers that are not the measured ones.

## What the numbers say

**A write costs about twice a read** — 7,834 against 3,771 stroops. The difference is
almost entirely storage: 6,952 of the write's non-refundable fee against 3,631 for the
read, because `record` has to pay for the ledger write, its TTL extension and the event
it publishes, and `sequence` only reads.

**A cross-contract call adds roughly 750 stroops** — `caller.observe` at 4,514 stroops
against `callee.sequence` at 3,771 for the same read, and `caller.record_via` at 8,599
against `callee.record` at 7,834 for the same write. That is the cost of the extra frame
and the interface it goes through, and it is the price of the property this project
exists to observe: the call is real, so the edge the engine recovers is real.

**Nested authorization is close to free** — 8,599 against 7,834, about 765 stroops. Worth
stating because the intuition is the other way: the extra authorization entry the callee
requires on the nested path is a few hundred bytes, not a second execution.

**The absolute cost is negligible.** The most expensive path in this project costs
0.00086 XLM. Nothing here is worth optimising on the fee, which is the useful conclusion:
the work in this repository that matters is whether an analysis is *true*, and the
contracts are cheap enough that no design decision in them was made to save a stroop.

## The optimisation that was made, and what it is for

Both crates build with `opt-level = "z"`, `lto = true`, `codegen-units = 1`,
`panic = "abort"` and `overflow-checks = true`. That profile was chosen for deployability
rather than for the fee: a smaller module is cheaper to upload and simpler to audit, and
the resource floor for a WASM contract is dominated by the host's fixed overhead rather
than by instruction count at this size. The one non-negotiable line is
`overflow-checks = true` — it costs instructions and buys defined behaviour on overflow,
and a benchmark is not a reason to turn it off.

There is no claim here that these are the smallest possible numbers. They are the
measured ones, and the honest use of them is as a baseline: a future change that pushes
`record_via` from 8,599 stroops to 20,000 is worth explaining, and one that moves it by
a hundred is noise on a shared network.

## What this measurement is not

- **Not a benchmark of a contract in production.** The reference contract exists to
  produce a cross-contract call the engine can observe. It is two functions and a
  counter, and its cost says nothing about the cost of a contract with real logic.
- **Not a gas limit.** There is no limit here to compare against, because there is no gas.
- **Not reproducible to the stroop.** The fee depends on the network's current read/write
  pricing and on which ledger entries the footprint touched, so a re-run at a later
  ledger can differ. What is stable is the *relationship* between the rows - a write
  costing about twice a read, a nested call adding a few hundred stroops - and that is
  what the discussion above relies on.
- **Not a security or correctness claim.** Cheap says nothing about safe.
