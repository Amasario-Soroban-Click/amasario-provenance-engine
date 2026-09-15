#!/usr/bin/env bash
#
# Check every example under `examples/`.
#
# There are two kinds of example and they are checked differently, because only one of
# them can be checked the same way everywhere.
#
# An offline example reads the generated corpus and its exact stdout is committed as
# `expected.out` beside it. This script runs it and compares the bytes, so an example that
# stops working fails here rather than being noticed by a reader who copied it.
#
# An endpoint example analyses something only a network can describe, so it cannot run in
# CI and does not pretend to. Its command script honours `AMASARIO_RPC`, and this script
# substitutes an address that refuses connections. The run must then exit 69 - the
# NETWORK category - which verifies the one thing that is verifiable without a chain:
# every flag in the documented command line parses and reaches the network layer. An
# unknown flag would exit 2 before any request was made, so a 2 here is a failure.
#
# Usage:
#   scripts/check-examples.sh
#
# Environment:
#   AMASARIO_DEAD_ENDPOINT   The address endpoint examples are pointed at. Defaults to a
#                            port nothing listens on. It must refuse connections rather
#                            than resolve and time out, or the check would be slow and its
#                            result would depend on the host's network configuration.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
cd "${repo_root}"

dead_endpoint="${AMASARIO_DEAD_ENDPOINT:-http://127.0.0.1:9/}"
status=0
offline=0
endpoint=0

fail() {
  printf '\033[31mFAIL\033[0m %s\n' "$1" >&2
  status=1
}

step() { printf '\033[1m==> %s\033[0m\n' "$1"; }

# The binary is built once here rather than by each example's own `cargo run`, so that a
# compile failure is reported once and clearly instead of as nine confusing diffs.
step "building the CLI"
cargo build -q -p amasario-cli

for dir in examples/*/; do
  name="$(basename "${dir}")"
  command="${dir}command"

  if [ ! -f "${dir}README.md" ]; then
    fail "${name} has no README.md"
    continue
  fi

  if [ ! -f "${command}" ]; then
    fail "${name} has no command script"
    continue
  fi

  # A script that is not valid bash is an example nobody can run.
  if ! bash -n "${command}"; then
    fail "${name}/command is not valid bash"
    continue
  fi

  expected="${dir}expected.out"
  actual="$(mktemp)"

  if [ -f "${expected}" ]; then
    offline=$((offline + 1))
    if ! bash "${command}" >"${actual}" 2>/dev/null; then
      fail "${name}/command exited non-zero; it is declared offline and must run anywhere"
      rm -f "${actual}"
      continue
    fi
    if ! diff -u "${expected}" "${actual}" >/dev/null; then
      fail "${name}: output does not match the committed expected.out"
      diff -u "${expected}" "${actual}" | head -40 >&2
      rm -f "${actual}"
      continue
    fi
    printf '  ok  %s (offline, output matches expected.out)\n' "${name}"
  else
    endpoint=$((endpoint + 1))
    set +e
    AMASARIO_RPC="${dead_endpoint}" bash "${command}" >"${actual}" 2>/dev/null
    code=$?
    set -e
    if [ "${code}" -ne 69 ]; then
      fail "${name}: expected exit 69 (NETWORK) against an unreachable endpoint, got ${code}"
      if [ "${code}" -eq 2 ]; then
        fail "${name}: exit 2 means a flag in the documented command line does not parse"
      fi
      rm -f "${actual}"
      continue
    fi
    printf '  ok  %s (endpoint, flags parse and a transport failure is classified)\n' "${name}"
  fi

  rm -f "${actual}"
done

printf '\n%d offline example(s) reproduced, %d endpoint example(s) verified.\n' \
  "${offline}" "${endpoint}"

if [ "${status}" -ne 0 ]; then
  printf '\033[31mExample checks failed.\033[0m\n' >&2
  exit 1
fi

printf '\033[32mEvery example checks out.\033[0m\n'
