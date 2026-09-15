# Contributing to `amasario-provenance-engine`

Thank you for considering a contribution. This is infrastructure that other people will
depend on to make deployment and release decisions, so the bar is not "does it work" but
"can a reader verify it works".

## Before you start

Read the two documents that answer most questions before they are asked:

- [`README.md`](README.md) - what the engine is, what it refuses to claim, and the
  invariants `amasario-core` enforces.
- [`docs/architecture.md`](docs/architecture.md) - the crate boundaries and the pipeline.

If your change touches what the engine *means* rather than how it behaves - a relationship,
an evidence class, a confidence level - it belongs in
[`amasario-provenance-spec`](https://github.com/Amasario-Soroban-Click/amasario-provenance-spec)
first. The engine consumes the specification; it does not define it.

## Getting set up

```console
git clone https://github.com/Amasario-Soroban-Click/amasario-provenance-engine
cd amasario-provenance-engine
git clone https://github.com/Amasario-Soroban-Click/amasario-provenance-spec .amasario-spec
scripts/install.sh          # or: cargo build --workspace --all-features
```

The toolchain is pinned in `rust-toolchain.toml`. `rustup` reads it automatically, so the
compiler you get is the one CI uses.

## The checks

Run the whole pipeline before opening a pull request:

```console
scripts/run-ci.sh
```

That mirrors `.github/workflows/ci.yml` and `.github/workflows/security.yml` step for
step, including the dependency-policy check, so that a `deny.toml` or `Cargo.lock`
problem fails here rather than on the pull request. If you want the pieces:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo test --workspace --all-features
scripts/check-examples.sh
AMASARIO_SPEC_DIR=.amasario-spec cargo test --workspace --all-features -- --include-ignored
AMASARIO_SPEC_DIR=.amasario-spec scripts/validate-profile.sh
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo deny check
cargo audit --deny warnings
```

`cargo-deny` and `cargo-audit` are release binaries rather than cargo plugins this
repository builds, so install them yourself (`cargo install cargo-deny cargo-audit
--locked`). `scripts/run-ci.sh` reports both as skipped when they are absent instead of
reporting the gate as passed.

CI fails on a formatting difference, a clippy warning, a failing test, a broken
documentation link, a broken example and an invalid profile. None of those is negotiable,
and none of them is a matter of taste: `cargo fmt` is the formatter, so there is no style
debate to have.

`scripts/check-examples.sh` is worth knowing about before you change output. The examples
under [`examples/`](examples/) are executable documentation, and the offline ones carry
their exact stdout as `expected.out`. A change to a renderer therefore shows up as a
diff there, which is deliberate: it is a change a reader of the documentation needs to
know about, and regenerating `expected.out` is how you say you meant it.

## What a good change looks like

### Nothing is claimed that the code does not do

This is the rule the rest of them serve. A doc comment that says a function verifies a
digest when it compares strings is worse than no doc comment, because a reader will trust
it. `#[must_use]`, `#[deny(missing_docs)]` and the review checklist exist to make an
untrue statement hard to land, not to add ceremony.

### A failure is never an absence

The engine's strongest requirement is that a network failure stays distinguishable from
"nothing was found". When you add a code path that returns a collection, ask what it
returns when the lookup failed, and make sure the answer is not an empty collection. This
is the single most common way to break this codebase.

### Evidence travels with the claim

A dependency without a basis and a citation is not a low-confidence dependency; it is not
a dependency. Verification without an assessment a reader can argue with is a verdict
rather than a finding. If your change produces a claim, it produces the evidence beside it.

### A bounded result says it was bounded

Traversal has a depth bound, an event scan has a page bound, a transaction read has a count
bound. When one of them stops early, the result must carry the truncation and the reason.
A bounded result that presents itself as complete is the failure this rule prevents.

### Determinism

Two runs over the same input, specification version, observation boundary and engine
version must produce byte-identical output. Practically that means: no `HashMap` iteration
where the order reaches output, no clock read inside an analysis, no ordering left to the
filesystem. The CLI is where the clock is read; the engine takes the timestamp as an
argument.

## Adding a command

A new CLI command is an entry in `crates/amasario-cli/src/main.rs`, a module under
`src/commands/`, and an exit code from `errors.rs` derived from an engine category rather
than invented. Keep the analysis in a library crate: the CLI parses, drives, renders and
chooses a code, and holds no analysis logic of its own. If you find yourself writing an
algorithm in `commands/`, it belongs in the crate that owns that layer.

## Adding a dependency

Ask three questions in the pull request description:

1. What does it do that the standard library or an existing dependency does not?
2. Is it maintained, and by whom?
3. What does it pull in transitively?

Significant dependencies are documented with their reason in the workspace manifest's
`[workspace.dependencies]`, next to the version. A dependency that cannot be explained in
a sentence is usually one that should not be added.

Do not add a dependency to get a one-line helper. The engine has a deliberately small
dependency set, and a package that exists to save ten lines is a supply-chain surface
bought for nothing.

## Adding a test

- Unit tests live beside the code they test and are named as sentences describing the
  property, not the function: `a_failure_is_not_recorded_as_an_absence`, not `test_run`.
- A test that asserts a bug exists must say why, so that a future reader does not delete
  it as wrong.
- Integration tests live in `integration-tests/`, and negative cases are as important as
  positive ones. The engine's failures are part of its contract.
- A fixture that represents a real network response must say where it came from. An
  invented fixture that looks real is how a test comes to assert behaviour that never
  occurs.

## Pull requests

Use the template. In the description, state:

- **What changed and why.** The "why" is what a reader six months from now needs.
- **How you verified it.** The commands you ran and what they printed. "Tests pass" without
  the command is not verification.
- **What you did not do.** A known gap stated plainly is a gap; a known gap left implicit
  becomes a bug report.

Keep each pull request to one coherent change. A pull request that fixes a bug and
refactors a module is two pull requests, and the reviewer will ask for it to be split,
because a review that cannot tell which lines fix the bug cannot tell whether the bug is
fixed.

## Commit messages

Explain the "what" and the "why", not the "how" - the diff is the how. A first line under
72 characters, a blank line, then the explanation if one is needed.

## Reporting a bug

Use the bug report template. Include the command, the version, the network, and the output
you got. Do not paste an API key, a token or a secret; if the bug involves one, describe
its shape rather than its value.

## Security

Do not open a public issue for a security problem. See [`SECURITY.md`](SECURITY.md).

## Code of conduct

Be straightforward and be kind. Disagree about the code, not about the person. A review
comment explains the problem it is pointing at; "this is wrong" without a reason is not a
review, it is a vote.

## Licence

By contributing you agree that your contribution is licensed under the Apache License,
Version 2.0, as the repository is. See [`LICENSE`](LICENSE).
