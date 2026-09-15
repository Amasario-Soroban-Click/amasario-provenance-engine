#!/usr/bin/env bash
#
# Run the read-only analysis commands against a contract on a live network.
#
# This is a smoke test, not a test suite: it proves that the CLI reaches a real
# endpoint, that the engine's network identity check passes against the chain it was
# pointed at, and that each command renders. Its outputs are not asserted, because a
# live network is not deterministic - asserting them would either fail constantly or
# assert nothing.
#
# It performs only read-only analysis. Nothing here submits a transaction, and the
# commands it runs have no mutating mode to submit one with.
#
# Usage:
#   scripts/test-testnet.sh <CONTRACT_ID> [network]
#
# Environment:
#   AMASARIO_BIN   The binary to run. Defaults to the workspace build, then PATH.

set -euo pipefail

if [ $# -lt 1 ]; then
  cat >&2 <<'EOF'
usage: scripts/test-testnet.sh <CONTRACT_ID> [network]

  CONTRACT_ID   A Soroban contract address, e.g.
                CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4
  network       futurenet, testnet (default) or mainnet. Mainnet also needs
                --rpc, which this script passes through from AMASARIO_RPC.
EOF
  exit 2
fi

contract="$1"
network="${2:-testnet}"

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

bin="${AMASARIO_BIN:-}"
if [ -z "${bin}" ]; then
  if [ -x "${repo_root}/target/debug/amasario" ]; then
    bin="${repo_root}/target/debug/amasario"
  elif [ -x "${repo_root}/target/release/amasario" ]; then
    bin="${repo_root}/target/release/amasario"
  elif command -v amasario >/dev/null 2>&1; then
    bin="$(command -v amasario)"
  else
    echo "test-testnet.sh: no amasario binary found." >&2
    echo "Build one with 'cargo build -p amasario-cli' or set AMASARIO_BIN." >&2
    exit 1
  fi
fi

# The endpoint override, when one is supplied. Required for mainnet, where SDF
# operates no public RPC endpoint.
rpc_args=()
if [ -n "${AMASARIO_RPC:-}" ]; then
  rpc_args=(--rpc "${AMASARIO_RPC}")
fi

# One scratch directory for everything, removed on exit so a smoke test leaves
# nothing behind.
workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

step "inspect ${contract} on ${network}"
"${bin}" inspect --contract "${contract}" --network "${network}" "${rpc_args[@]}"

step "discover"
"${bin}" discover --contract "${contract}" --network "${network}" "${rpc_args[@]}"

step "dependencies (depth 2, event scan)"
"${bin}" dependencies --contract "${contract}" --network "${network}" \
  --depth 2 --scan-events "${rpc_args[@]}"

step "graph (DOT)"
"${bin}" graph --contract "${contract}" --network "${network}" \
  --format dot --output "${workdir}/graph.dot" "${rpc_args[@]}"

step "provenance"
"${bin}" provenance --contract "${contract}" --network "${network}" "${rpc_args[@]}"

step "verify"
# Not a gate here: this smoke test is checking that the command runs against a live
# endpoint. It reports the status without requiring one.
"${bin}" verify --contract "${contract}" --network "${network}" "${rpc_args[@]}"

step "snapshot create"
"${bin}" snapshot create --contract "${contract}" --network "${network}" \
  --output "${workdir}" --depth 2 "${rpc_args[@]}"

snapshot_file="$(find "${workdir}" -maxdepth 1 -name '*.json' -print -quit)"
if [ -z "${snapshot_file}" ]; then
  echo "test-testnet.sh: the snapshot command wrote no file, which is a defect." >&2
  exit 1
fi

step "snapshot show"
"${bin}" snapshot show --input "${snapshot_file}"

step "report (Markdown)"
"${bin}" report --contract "${contract}" --network "${network}" \
  --format markdown --output "${workdir}/report.md" "${rpc_args[@]}"

step "export (GraphML)"
"${bin}" export --input "${snapshot_file}" --format graphml --output "${workdir}/graph.graphml"

printf '\n\033[1;32mSmoke test passed.\033[0m Artifacts were written to a temporary directory and removed.\n'
echo "The results were not asserted: a live network is not deterministic. Re-run against"
echo "a recorded fixture, or a local network, when a stable assertion is needed."
