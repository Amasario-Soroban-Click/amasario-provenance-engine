#!/usr/bin/env bash
#
# Asserts the reference contract's two manifests carry the engine's lint policy.
#
# # The problem this exists for
#
# The two halves under `reference-contract/` are their own build roots - the caller
# cannot be compiled until the callee's module exists - so they cannot say
# `[lints] workspace = true`, and their lint tables are therefore copies of the ones in
# the root `Cargo.toml`.
#
# A copy is a file that drifts. Tightening the workspace policy - adding a lint, raising
# one from warn to deny - would apply to the twelve member crates and silently skip the
# two contract crates, so the one artefact this repository deploys would be the one
# compiled under the weakest policy. Nothing about a green `CI` run would say so, and
# nothing about the contract sources would either.
#
# So the duplication is asserted instead of trusted. Every key and value in
# `[workspace.lints.rust]` and `[workspace.lints.clippy]` has to appear identically in
# `[lints.rust]` and `[lints.clippy]` of both manifests, with one named exception
# documented below.
#
# # The exception, and why it is named here rather than left to the manifests
#
# `unsafe_code` is `forbid` in the workspace and `deny` in both contract crates. That is
# not a preference: `forbid` cannot be overruled by an inner `allow`, and
# `#[contractimpl]` expands to an `#[allow(unsafe_code)]` through `soroban-sdk`'s own
# `ctor_entry` support macro, so a crate that uses the attribute does not compile under
# `forbid` at all - `error[E0453]: allow(unsafe_code) incompatible with previous forbid`.
#
# `missing_docs` is `allow` where the workspace says `warn`, for a second reason from the
# same place: `#[contract]` generates a client type whose fields and associated functions carry
# no documentation, and no `allow` placed on the item reaches them - the lint is reported
# at the `#[contract]` line, and an `allow` there takes the count from five errors to four.
# The callee's manifest carries both findings and what stands in for the documentation.
#
# Both exceptions are applied to the workspace's table before comparing, so the workspace
# remains the single source of truth: every other key is still compared exactly, and a
# lint added to the workspace still has to be added to both contracts. Each is also
# checked for staleness - if the workspace stops declaring the key, or adopts the level
# the contract already holds, this script fails and names the line to delete, rather than
# carrying a difference that no longer means anything.
#
# # What it does not do
#
# It compares the tables as lists of normalised `key=value` lines, which is what the
# cargo lint tables are. It is deliberately not a TOML parser: a parser is a dependency,
# and the point of this check is to be runnable from a shell on the runner image with no
# toolchain and no network.
#
# # Usage
#
#   scripts/check-reference-lints.sh
#
# Exits zero when both copies match the workspace's tables, and non-zero otherwise,
# printing the differing lines. `.github/workflows/reference.yml` runs it.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly root

# The entries a reference-contract manifest holds at a different level than the workspace,
# each as the `key=value` the contracts carry. Everything not named here is compared
# exactly. Both entries are checked for staleness before they are applied, so neither a
# redundant exception nor one for a key that has gone away survives a run.
readonly -a contract_exceptions=(
  # `forbid` cannot be overruled by an inner `allow`, and `#[contractimpl]` expands one
  # through the SDK's `ctor_entry` macro.
  'unsafe_code="deny"'
  # `#[contract]` generates an undocumented client, and no outer `allow` reaches its
  # fields or methods. The generated interface is asserted by
  # `integration-tests/reference/` instead of described.
  'missing_docs="allow"'
)

# One table's entries, normalised so that whitespace and ordering cannot make two
# identical policies look different. The header line is dropped because the tables are
# named differently at their two sites (`workspace.lints.rust` against `lints.rust`), and
# the keys are what has to agree.
extract() {
  local table="$1"
  local file="$2"
  local found

  # Whitespace is stripped per line rather than with `tr -d '[:space:]'`, which also
  # removes the newlines and would report a one-line policy difference as one long line.
  # `sed` sees the pattern space, which never contains the line separator.
  #
  # The key pattern admits digits, and that is not decoration: `rust_2018_idioms` is a
  # key, and a pattern of `[[:lower:]_]+` skips it silently. Which is what happened -
  # the first version of this script passed while never comparing the one entry whose
  # priority has to be stated explicitly, so a contract that dropped it would have been
  # checked against a table that no longer contained the key.
  found="$(sed -n "/^\\[${table}\\]/,/^\\[/p" "$file" \
    | grep -E '^[[:lower:]][[:lower:][:digit:]_]*[[:space:]]*=' \
    | sed 's/[[:space:]]//g' \
    | sort)"

  if [ -z "$found" ]; then
    echo "no entries found under [$table] in $file" >&2
    echo "the table is either missing or empty, and either way the policy is not applied" >&2
    exit 1
  fi

  printf '%s\n' "$found"
}

# Rewrites those entries in the workspace's table to the levels a contract can hold. A
# no-op for the clippy table, which has no exceptions, and a no-op for any key the
# workspace stops declaring - which the staleness check below catches first.
with_contract_exceptions() {
  local table="$1"
  local exception
  local script=""

  if [ "$table" != "rust" ]; then
    cat
    return
  fi

  for exception in "${contract_exceptions[@]}"; do
    script+="s|^${exception%%=*}=.*|${exception}|;"
  done

  sed "$script"
}

# Each exception is checked for staleness before it is applied, because the failure mode
# of a stale exception is silence: it would keep passing while documenting a difference
# that no longer exists. Two ways for one to go stale - the workspace dropping the key,
# or the workspace adopting the level the contract already holds - and both are errors
# that say which line to delete.
for exception in "${contract_exceptions[@]}"; do
  key="${exception%%=*}"
  workspace_entry="$(extract "workspace.lints.rust" "$root/Cargo.toml" \
    | grep "^${key}=" || true)"

  if [ -z "$workspace_entry" ]; then
    echo "the workspace no longer declares ${key} under [workspace.lints.rust]," >&2
    echo "so the exception this script carries for reference-contract/ is stale." >&2
    echo "Delete that entry from the contract_exceptions list in this script and" >&2
    echo "the matching line from both contract manifests." >&2
    exit 1
  fi

  if [ "$workspace_entry" = "$exception" ]; then
    echo "the workspace now sets ${exception} itself, so the exception this script" >&2
    echo "carries for reference-contract/ is redundant and stale. Delete that entry" >&2
    echo "from the contract_exceptions list in this script and bring both manifests" >&2
    echo "back into line with the workspace." >&2
    exit 1
  fi
done

status=0

for table in rust clippy; do
  workspace_table="workspace.lints.${table}"
  expected="$(extract "$workspace_table" "$root/Cargo.toml" | with_contract_exceptions "$table")"

  for half in callee caller; do
    manifest="$root/reference-contract/${half}/Cargo.toml"
    actual="$(extract "lints.${table}" "$manifest")"

    if ! diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") >&2; then
      echo >&2
      echo "reference-contract/${half}/Cargo.toml does not carry the workspace policy." >&2
      echo "Left is [$workspace_table] in Cargo.toml, with the documented" >&2
      echo "unsafe_code exception applied; right is [lints.${table}] in the manifest." >&2
      echo "Copy the workspace table across, or change both." >&2
      status=1
    fi
  done
done

if [ "$status" -ne 0 ]; then
  exit "$status"
fi

echo "both reference-contract manifests carry the workspace lint policy"
