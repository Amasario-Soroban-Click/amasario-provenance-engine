# Example: what changed between two observation boundaries

A snapshot records what was observed at one boundary. A diff compares two of them and
says what is different, without saying whether the difference matters.

## The command

```console
cargo run -q -p amasario-cli -- diff \
  --before fixtures/snapshots/testnet-alpha-before.json \
  --after  fixtures/snapshots/testnet-alpha-after.json
```

Runs offline. Both snapshots are engine-generated, and both are the same contract at two
boundaries - which is the only case in which a diff is meaningful.

## What the output shows

Four changes: one relationship added, three impact-surface changes. Each entry is
four lines, and the four lines are deliberate:

```
ADDED [RELATIONSHIP_ADDED] CONTRACT:CCQ2... at /dependencies/CONTRACT:CCQ2... -INVOCATES-> CONTRACT:CDKN...
  because     the relationship is asserted in the later capture and was not asserted in the earlier one
  after       "CONTRACT:CCQ2...-INVOCATES->CONTRACT:CDKN..."
```

- The **subject** says which entity the change is about.
- The **category** (`RELATIONSHIP_ADDED`, `WASM_IDENTITY_CHANGED`,
  `IMPACT_SURFACE_CHANGED` and the rest) is from the specification's change taxonomy,
  not a free-text label.
- The **reason** is mandatory. A diff is the artefact most likely to be consumed without
  review, so a reviewer needs something to reject an entry on. "This differs" is not
  enough; "this is asserted here and not there" is.
- The **before/after values** are printed for any change with an ordering, so the reader
  is not sent to two files to find out what moved.

The run ends with a statement of what the difference implies and what it does not:

> this difference changes the dependency or provenance surface, so an impact analysis
> from the after-state is warranted
>
> A difference here is a change in what was observed, not an assessment of whether the
> change is safe.

The second line is the one that matters. A new dependency edge is a change in the
observation, and the engine has no basis on which to call it a regression.

## Why the content digest, not the timestamp

Each snapshot carries a `contentDigest` computed over its canonical form with the
declared volatile fields - the capture timestamp among them - excluded. Two captures of
an unchanged contract therefore compare equal, which is what makes a scheduled diff
useful instead of an alert on every run.

When the digests differ the diff proceeds; when they are equal the diff is empty, and an
empty diff is a result rather than a failure. The command exits `0` either way.

## Creating snapshots

```console
cargo run -q -p amasario-cli -- snapshot create \
  --contract CCQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2DINBUGQ2CNSG \
  --network testnet --output snapshots/

cargo run -q -p amasario-cli -- snapshot show --input snapshots/<file>.json
```

`snapshot create` requires a network; `show` and `diff` do not. A snapshot written by a
different specification version is refused rather than reinterpreted, because a field
whose meaning changed would otherwise be compared as though it had not.
