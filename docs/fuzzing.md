# Fuzzing

The engine parses input it did not write. A WebAssembly module is whatever a network
endpoint served, a recorded RPC response is whatever that endpoint said, a snapshot is a
file that may have been edited, truncated or written by an older engine, and a profile is
a document a user may have hand-modified. The fuzz targets exist because those are the
paths where a malformed input is expected rather than hypothetical.

The invariant every target checks is the same one `SECURITY.md` states: **a bad input
produces a classified error, never a panic and never an unbounded loop.** A panic would
turn a hostile response into a crash of the tool that was supposed to report on it.

Nothing here executes the code it reads. The engine parses WebAssembly modules; it never
runs them, and a target that caused execution would be a bug rather than a finding.

## The targets

Five targets live under `fuzz/`, each a `[[bin]]` of a single package in a workspace of its
own so that the engine's `cargo test --workspace` stays a plain stable-toolchain build and
a fuzzer can never become a dependency of a published crate. Each one asserts the engine's
documented contracts on every input rather than only checking that no panic occurred - a
target that stopped at "it did not crash" would let a regression through as long as it
failed politely.

The layout is the one cargo-fuzz documents, and it is a requirement rather than a
convention. `cargo fuzz` reads `fuzz/Cargo.toml` and requires it to be a package carrying
`cargo-fuzz = true` under `[package.metadata]`, with each target a `[[bin]]` pointing into
`fuzz_targets/`. A target added under any other shape is not printed by `cargo fuzz list`
and is therefore not run - and it fails silently, because the scheduled job iterates
exactly what that command prints. Two related details are easy to lose: every command here
is run from the repository root, since `cargo fuzz` resolves `fuzz/` relative to the
working directory; and `fuzz/Cargo.toml` ends its own `[workspace]`, because a package
inside the engine's workspace root is otherwise a member of it, and the engine's stable
`-D warnings` build would try to compile `libfuzzer-sys` and the sanitizer flags.

| Target | Explores | Asserted beyond "no panic" |
| --- | --- | --- |
| `provenance-fuzzer` | The module decoder, `digest_of`, `verify_digest`, and the `Digest` / `TransactionHash` constructors | Decoding is total; a decoded module satisfies `looks_like_module`, so the pre-check does not disagree with the decoder; the digest is a function of its input; a module verifies against its own digest and not against a forged one |
| `dependency-fuzzer` | Candidate construction, `resolve`, and the bounded transitive closure | Every published dependency carries evidence, a reason and a class; no self-dependency; a transitive entry is at least two hops from its subject and carries the path that establishes it; a relationship never appears in both partitions; a truncated result says why it stopped |
| `graph-fuzzer` | Assembly through `add_edge`, forward and reverse walks, bounded path search, cycle detection | An accepted edge is present and a refused edge changed nothing, checked against the node and edge counts; no walk reports an entity deeper than its bound; a bounded search never exceeds its cap; `has_cycle` agrees with `find_cycles`; a reported cycle names at least two entities |
| `impact-fuzzer` | Bounded propagation over assembled graphs, including cyclic ones | The analysis terminates and respects its bounds; a truncated analysis says why and does not call itself conclusive; every finding's route and hop depth agree; each step moved in a direction its relationship actually declares, so a propagation cannot walk the wrong way and return a plausible, wrong answer |
| `snapshot-fuzzer` | Decoding, round-tripping, canonicalisation, and comparison | A snapshot that parses validates; a written snapshot reads back equal to itself; canonicalisation does not depend on collection order, so a reordered copy of a snapshot is reported as identical; a produced diff satisfies its own rules and counts the changes it carries |

## Running them

`cargo fuzz` drives `-Zsanitizer`, which is a nightly-only flag, so it needs the nightly
toolchain in addition to the target itself. The engine's pinned toolchain in
`rust-toolchain.toml` is deliberately not enough - the fuzz workspace is a second
workspace for exactly this reason.

```console
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
cargo +nightly fuzz list
cargo +nightly fuzz run graph-fuzzer
```

Run them from the repository root. `cargo fuzz list` is worth running before anything else
because it is the same question the scheduled job asks: if it prints fewer targets than
`fuzz/Cargo.toml` declares, the missing ones are not being fuzzed and nothing will say so.

A run without a time limit continues until it finds something or is interrupted. A
bounded run is what CI does:

```console
cargo +nightly fuzz run graph-fuzzer -- -max_total_time=300 -rss_limit_mb=4096
```

Every target is expected to run clean. A target that finds a crash is a reported bug, not
a flaky job: see `SECURITY.md` for how to report one privately.

## Reading a failure

A target that finds something writes the offending input to
`fuzz/artifacts/<target>/`, and the assertion message names the property that failed
rather than merely the line. To reproduce it:

```console
cargo +nightly fuzz run graph-fuzzer fuzz/artifacts/graph-fuzzer/<file>
```

If the input is small enough to read, the useful next step is a unit test in the crate
that owns the invariant, written as a sentence about the property - an assertion whose
message says what went wrong is what `CONTRIBUTING.md` asks for. The corpus entry that
found it stays in `fuzz/corpus/<target>/`, which the scheduled run caches between runs so
the input is replayed without the directory being committed: a corpus is machine state,
and a regression that has to survive belongs in a test rather than in a compiled blob.

## What fuzzing does not establish

- **It is not a proof.** A clean run says these inputs did not reach a failure, not that
  no input can. Coverage-guided fuzzing finds the defects it can reach in the time it is
  given.
- **It is not a security audit, and no result here is a security opinion.** The same
  boundary `SECURITY.md` draws applies: the targets check that the engine survives
  malformed input, not that any contract, dependency or endpoint is trustworthy.
- **It does not reach the network.** Every target builds its inputs from bytes. The paths
  that require an endpoint are covered by the recorded fixtures under `fixtures/` and by
  the scheduled live test, not here.

## The scheduled run

`.github/workflows/fuzz.yml` runs every target on a schedule - daily at 03:00 UTC - and
on demand, with a per-target time budget (300 seconds by default, settable on a manual
dispatch). The accumulated corpus is cached between runs, so each scheduled run starts
from everything the previous ones discovered rather than from the committed seeds alone.

A run that finds something uploads `fuzz/artifacts/` as a workflow artifact rather than
only failing, because the reproducer is the useful output and the exit code is not. The
job is deliberately not part of the pull-request gate: a ninety-second run on every pull
request finds shallow defects while costing every contributor two minutes, and the shape
of the trade is the same one `docs/ci-integration.md` describes for the live network
test.
