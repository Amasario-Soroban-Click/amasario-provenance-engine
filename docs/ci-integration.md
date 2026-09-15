# CI integration

Two things live here: how *this* repository's CI is wired, and how to wire the engine into
*your* CI.

## The quickest way in: the composite action

The repository publishes a composite action, so adopting the engine does not require
working out how to build it, where the binary lands, or which flags a command takes:

```yaml
jobs:
  provenance:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      - uses: Amasario-Soroban-Click/amasario-provenance-engine/.github/actions/amasario@v1
        with:
          contract: CABC...
          network: testnet
          command: dependencies
          args: --scan-events --depth 2
          output: dependency-report.md
```

It builds the engine from the ref you pinned — so the action cannot run an engine other
than the one at that ref — writes the rendered document to `output`, and appends it to the
job summary so the finding is visible without downloading an artifact. It exposes `report`
and `binary` as outputs.

| Input | Default | Notes |
| --- | --- | --- |
| `contract` | *(required)* | The address to analyse. |
| `command` | `dependencies` | One of `inspect`, `discover`, `provenance`, `dependencies`, `graph`, `impact`, `snapshot`, `verify`, `report`. |
| `network` | `testnet` | `mainnet` also needs `rpc`. |
| `args` | *(empty)* | Passed through as one string. `--scan-events` is what finds cross-contract calls; the action warns when a command that scans events ran without it. |
| `format` | `markdown` | `text`, `json`, `markdown`, `dot` or `junit`. |
| `output` | `amasario-report.md` | Relative to the caller's workspace. |
| `require` | *(empty)* | For `verify`: `verified`, `partially` or `unverified`. |
| `claimed-digest` | *(empty)* | For `verify`: a SHA-256 digest to compare against the network's. |
| `rpc`, `horizon`, `passphrase`, `network-name` | *(empty)* | Endpoint and identity overrides. |
| `toolchain` | `true` | Install the pinned toolchain before building. |

The action runs read-only analysis, and the engine has no mutating mode for it to reach.

## Gating a release on provenance

Without the action, build the engine yourself and call it:

```yaml
- name: Install the engine
  run: cargo install --path .  # or: use a prebuilt binary

- name: Verify the deployed contract
  env:
    CONTRACT_ID: ${{ vars.CONTRACT_ID }}
  run: amasario verify --contract "$CONTRACT_ID" --network testnet --require partially
```

With `--format json`, read `scope` before trusting the status. `scope.checked` and
`scope.notChecked` state what was compared and what was not, which is the difference
between internal consistency and an independent attestation — see
[verification.md](verification.md).

`amasario verify` exits `1` when the outcome is `CONFLICTING` or falls short of `--require`,
so a job fails without parsing anything. The full status and exit-code tables are in
[cli.md](cli.md) and [verification.md](verification.md).

The `--require` level is a policy decision:

| Level | Use when |
| --- | --- |
| *(absent)* | You want any outcome other than a contradiction. |
| `unverified` | You want to be told when the evidence was never obtained. |
| `partially` | You want some evidence and no contradiction. |
| `verified` | You want every component checked. |

## Reporting into a test reporter

```yaml
- name: Write the report
  run: amasario report --contract "$CONTRACT_ID" --network testnet \
        --format junit -o junit.xml

- name: Publish results
  uses: actions/upload-artifact@v4
  with:
    name: amasario-junit
    path: junit.xml
```

A `CONFLICTING` status becomes a failed case, an `errors` section becomes an `<error>`, and
`UNKNOWN` becomes skipped. The mapping is in
[output-formats.md](output-formats.md).

## Diffing across a deployment

```yaml
- name: Capture before
  run: amasario snapshot create --contract "$CONTRACT_ID" --network testnet -o before.json

# ... the deployment ...

- name: Capture after
  run: amasario snapshot create --contract "$CONTRACT_ID" --network testnet -o after.json

- name: Diff
  run: amasario diff --before before.json --after after.json --format markdown
```

A diff that cannot be built — because its changes would share an identifier — exits
non-zero rather than writing a document that a consumer would misread. An incomparable
pair exits zero with `"comparable": false`, because two different contracts differing is
not an error.

## This repository's workflows

| Workflow | Runs |
| --- | --- |
| `ci.yml` | The gate: formatting, linting, the whole workspace test suite, and the conformance checks. |
| `rust.yml` | The Rust build matrix: toolchain, `clippy -D warnings`, `cargo doc` with warnings denied. |
| `integration.yml` | The nine end-to-end suites and the fixture-corpus currency check. |
| `testnet.yml` | The live smoke test, on a schedule and on demand. It never asserts a live result. |
| `fuzz.yml` | Builds the five fuzz targets and runs each for a bounded time. |
| `benchmark.yml` | Runs the six benchmarks and keeps a baseline for comparison. |
| `security.yml` | `cargo deny`, dependency auditing, and the lint policy. |
| `release.yml` | Builds and attaches release artefacts. |
| `docs.yml` | Checks that the documentation builds and that its examples are real. |

### The fixture-currency check

```bash
cargo run -p amasario-integration-tests --bin generate-fixtures -- --check
```

This writes nothing and reports whether the committed corpus equals what the builders
produce. It is the check that makes the fixtures meaningful: a model change that was not
followed by a regeneration fails the build instead of leaving a corpus that describes an
engine that no longer exists.

### Running the gate locally

```bash
scripts/run-ci.sh
```

It runs what the workflows run, in the same order, so a failure is reproducible before it
is pushed.

## Benchmarks

```bash
scripts/benchmark.sh                                   # every benchmark
scripts/benchmark.sh graph-traversal impact-analysis   # named ones
BENCH_ARGS=--quick scripts/benchmark.sh                # a fast pass
BENCH_ARGS='--save-baseline main' scripts/benchmark.sh # keep a baseline
```

Criterion writes its report under `target/criterion/`, and the baseline is the comparison
point for the next run. The six benchmarks and what each one measures are described in
[architecture.md](architecture.md) and the module documentation at the top of each file
under `benches/`.

A benchmark that measured a shorter walk under a longer walk's name would be worse than no
benchmark, so the longest cases are the engine's own depth ceiling and each one asserts
inside the measurement that it reached the end of its walk.

## Fuzzing

```bash
cargo fuzz run provenance-fuzzer     # with cargo-fuzz and a nightly toolchain
```

The five targets are described in [fuzzing](#fuzzing-what-each-target-explores) below, and
each file's module documentation states what it asserts. Without `cargo-fuzz` installed,
the targets still build and run:

```bash
cd fuzz
cargo build --workspace
./target/debug/graph-fuzzer -runs=100000
```

### What each target explores

| Target | Path | The case it is looking for |
| --- | --- | --- |
| `provenance-fuzzer` | Module decode, digest, digest verification, identity constructors | A byte string that makes the decoder panic, disagree with its own pre-check, or produce an unstable digest. |
| `dependency-fuzzer` | Candidate classification, resolution, closure | A candidate set that yields a published edge with no evidence, no reason, no class, or that lands in both partitions. |
| `graph-fuzzer` | Assembly, traversal, paths, cycles | A traversal that reports a depth beyond its bound, a path search beyond its cap, or two cycle implementations that disagree. |
| `impact-fuzzer` | Bounded propagation | A propagation that exceeds its bound, omits its truncation disclosure, or crosses a relationship in the direction it does not declare. |
| `snapshot-fuzzer` | Decode, canonicalise, digest, compare | A snapshot that fails to round-trip, a comparison that depends on collection order, or two changes that share an identifier. |

None of them only checks for a panic. A crash is a finding; an engine that is wrong without
crashing is the harder failure, so each target asserts the specification's own rules on
every input it produces — see [verification.md](verification.md) for the status rules and
[dependency-analysis.md](dependency-analysis.md) for the partition rule.

### Seeding

The fixture corpus is the natural seed: it holds a valid module, a module with no
interface, two byte sequences that are not modules, a real cycle, a disconnected graph and
a snapshot pair.

```bash
cargo fuzz run snapshot-fuzzer fixtures/snapshots/testnet-alpha-before.json
```

Commit any interesting corpus entry under `fuzz/<target>/corpus`, which is where a case
that found something belongs. `fuzz/artifacts`, `fuzz/coverage` and `fuzz/target` are
ignored, because libFuzzer's accumulated corpus is machine state rather than a reviewable
artefact.

### Reading a failure

A failure writes the input to `fuzz/artifacts/<target>/` and prints the assertion that
tripped. To reproduce it:

```bash
cargo fuzz run <target> fuzz/artifacts/<target>/<input>
```

The assertion text is written to say what the engine claimed and what it did, so the
failure is read as a specification violation rather than only as a stack trace. Fix the
engine or fix the assertion — never delete the input.

## The specification boundary

CI clones `amasario-provenance-spec` and points `AMASARIO_SPEC_DIR` at it. The conformance
tests then compare the engine's relationship vocabulary, basis ceilings, confidence
ordering and verification statuses against the specification's taxonomies. A taxonomy
change on the specification side therefore fails this repository's build rather than
silently altering an analysis, which is what stops the engine from redefining the model it
is supposed to implement.

A change that is genuinely needed on both sides is a two-repository change: the
specification version moves, and the engine's declared compatible range moves with it.
