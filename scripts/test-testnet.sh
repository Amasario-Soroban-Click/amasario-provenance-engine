#!/usr/bin/env bash
#
# Run the read-only analysis commands against a contract on a live network, and assert
# what the chain actually shows.
#
# This is a smoke test with assertions, and the assertions are the point. A version of
# this script that ran the commands and printed their output would go green while the
# flagship analysis returned nothing: that is precisely what happened, and it took a
# person reading a live response to notice. So the commands below are run with
# `--format json` and the documents are checked for the properties that must hold for
# any successful analysis of a reachable contract - evidence on every edge, a confidence
# level, an observation boundary - and for the one property that must hold for the
# committed target specifically: that a contract which calls another contract yields at
# least one verified `INVOCATES` edge.
#
# What is deliberately *not* asserted is anything that a live network can change under
# it: a ledger number, a transaction hash, an event count, a dependency's identity. A
# test that pinned those would fail for reasons unrelated to the change under review,
# which is how a live test becomes one that people learn to re-run.
#
# It performs only read-only analysis. Nothing here submits a transaction, and the
# commands it runs have no mutating mode to submit one with.
#
# Usage:
#   scripts/test-testnet.sh [CONTRACT_ID] [network]
#
#   With no CONTRACT_ID the committed target in scripts/live-target.env is used, so the
#   script has a real default rather than requiring an environment nobody set.
#
# Environment:
#   AMASARIO_BIN        The binary to run. Defaults to the release build, then the debug
#                       build, then PATH.
#   AMASARIO_RPC        Overrides the RPC endpoint. Required for mainnet, where SDF
#                       operates no public endpoint.
#   AMASARIO_ARTIFACTS  Where the outputs are written. Defaults to a temporary directory
#                       that is removed on exit; set it to keep them for a CI upload.
#   AMASARIO_LOOKBACK   How many ledgers of history the dependency scan covers. Defaults
#                       to the CLI's own default of 256, which is about a day.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

# The committed target. Read before the arguments are validated so that a run with no
# arguments is a plain `scripts/test-testnet.sh`.
if [ -f "${script_dir}/live-target.env" ]; then
  # shellcheck disable=SC1091
  . "${script_dir}/live-target.env"
fi

if [ $# -ge 1 ] && [ -n "$1" ]; then
  contract="$1"
else
  contract="${AMASARIO_TARGET_CONTRACT:-}"
fi

if [ -z "${contract}" ]; then
  cat >&2 <<'EOF'
usage: scripts/test-testnet.sh [CONTRACT_ID] [network]

  CONTRACT_ID   A Soroban contract address. Optional: when it is omitted the committed
                target in scripts/live-target.env is used.
  network       futurenet, testnet (default) or mainnet. Mainnet also needs --rpc,
                which this script passes through from AMASARIO_RPC.
EOF
  exit 2
fi

network="${2:-${AMASARIO_TARGET_NETWORK:-testnet}}"

bin="${AMASARIO_BIN:-}"
if [ -z "${bin}" ]; then
  if [ -x "${repo_root}/target/release/amasario" ]; then
    bin="${repo_root}/target/release/amasario"
  elif [ -x "${repo_root}/target/debug/amasario" ]; then
    bin="${repo_root}/target/debug/amasario"
  elif command -v amasario >/dev/null 2>&1; then
    bin="$(command -v amasario)"
  else
    echo "test-testnet.sh: no amasario binary found." >&2
    echo "Build one with 'cargo build --release -p amasario-cli' or set AMASARIO_BIN." >&2
    exit 1
  fi
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "test-testnet.sh: python3 is needed to check the analysis documents." >&2
  exit 1
fi

# The endpoint override, when one is supplied.
rpc_args=()
if [ -n "${AMASARIO_RPC:-}" ]; then
  rpc_args=(--rpc "${AMASARIO_RPC}")
fi

lookback_args=()
if [ -n "${AMASARIO_LOOKBACK:-}" ]; then
  lookback_args=(--lookback "${AMASARIO_LOOKBACK}")
fi

# A directory for the outputs. A caller that wants to keep them - CI, to upload them -
# names one; otherwise this makes a scratch directory and removes it on exit.
if [ -n "${AMASARIO_ARTIFACTS:-}" ]; then
  workdir="${AMASARIO_ARTIFACTS}"
  mkdir -p "${workdir}"
  keep_workdir=true
else
  workdir="$(mktemp -d)"
  keep_workdir=false
fi

if [ "${keep_workdir}" = false ]; then
  trap 'rm -rf "${workdir}"' EXIT
fi

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
ok() { printf '    \033[32m✓\033[0m %s\n' "$1"; }

# Checks one analysis document. The predicate is a python expression evaluated with the
# parsed document bound to `d`, and it must be true. Keeping the checks in python rather
# than grep is what makes them assertions about the document rather than about its
# formatting.
check() {
  local file="$1" predicate="$2" complaint="$3"
  python3 - "${file}" "${predicate}" "${complaint}" <<'PY'
import json, sys
path, predicate, complaint = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    with open(path) as handle:
        d = json.load(handle)
except (OSError, ValueError) as error:
    sys.exit(f"{path} is not a readable analysis document: {error}")
# The predicates are written here, not supplied by a caller, so the builtins they need
# are the ones the file grants: enough to quantify over a list and nothing else.
builtins = {"all": all, "any": any, "len": len, "isinstance": isinstance}
if not eval(predicate, {"d": d, "__builtins__": builtins}):  # noqa: S307
    sys.exit(f"{complaint}\n  in {path}")
PY
}

step "inspect ${contract} on ${network}"
inspect_doc="${workdir}/inspect.json"
"${bin}" inspect --contract "${contract}" --network "${network}" \
  --format json --output "${inspect_doc}" "${rpc_args[@]}"
check "${inspect_doc}" \
  "d['identity']['contractId'] == '${contract}'" \
  "the inspected contract is not the one that was asked for"
check "${inspect_doc}" \
  "d['identity']['executableKind'] in ('WASM', 'STELLAR_ASSET')" \
  "the identity names no executable kind, so nothing was resolved"
check "${inspect_doc}" \
  "d['boundary']['ledger'] > 0" \
  "the inspection reports no observation boundary, so it claims nothing about when it looked"
# The digest check is the one property that must hold for a WASM contract: the bytes
# the endpoint served must hash to the digest the endpoint records. For a Stellar asset
# contract there are no bytes to hash, and the identity says so instead.
check "${inspect_doc}" \
  "d['identity']['executableKind'] != 'WASM' or (d['identity'].get('wasmHash') and d['digestVerified'] is True)" \
  "a WASM contract was identified but its digest was not verified against its own bytes"
ok "identity resolved at ledger $(python3 -c "import json,sys;print(json.load(open('${inspect_doc}'))['boundary']['ledger'])")"

step "discover"
"${bin}" discover --contract "${contract}" --network "${network}" \
  --format json --output "${workdir}/discover.json" "${rpc_args[@]}"
check "${workdir}/discover.json" \
  "d['boundary']['ledger'] > 0" \
  "discovery reports no observation boundary"

step "dependencies (depth 2, event scan)"
dependencies_doc="${workdir}/dependencies.json"
"${bin}" dependencies --contract "${contract}" --network "${network}" \
  --depth 2 --scan-events "${lookback_args[@]}" \
  --format json --output "${dependencies_doc}" "${rpc_args[@]}"

# The structural invariants. These hold for every edge of every run, on any contract,
# because they are what the specification requires of a dependency claim.
check "${dependencies_doc}" \
  "d['id'].startswith('CONTRACT:')" \
  "the dependency set does not identify the contract it describes"
check "${dependencies_doc}" \
  "all(e.get('evidence') for e in d['edges'])" \
  "an edge carries no evidence, which the specification forbids: a dependency must name what supports it"
check "${dependencies_doc}" \
  "all(e.get('basis') for e in d['edges'])" \
  "an edge carries no basis, so nothing says how the relationship was established"
check "${dependencies_doc}" \
  "all(e['confidence']['level'] in ('VERIFIED','HIGH_CONFIDENCE','MEDIUM_CONFIDENCE','LOW_CONFIDENCE','UNKNOWN') for e in d['edges'])" \
  "an edge carries a confidence level outside the specification's taxonomy"
check "${dependencies_doc}" \
  "all(e['verificationStatus'] in ('VERIFIED','PARTIALLY_VERIFIED','UNVERIFIED','CONFLICTING','UNKNOWN') for e in d['edges'])" \
  "an edge carries a verification status outside the specification's taxonomy"
check "${dependencies_doc}" \
  "all(e['boundary']['ledger'] > 0 for e in d['edges'])" \
  "an edge reports no observation boundary, so the run cannot say when it observed it"

# The assertion the committed target exists for. See scripts/live-target.env: the target
# is a contract that calls another contract in ordinary activity, so a scan of it that
# yields no verified `INVOCATES` edge means either that the engine stopped recovering
# cross-contract calls or that the target went quiet. Both are worth failing for, and the
# message says which to check.
check "${dependencies_doc}" \
  "any(e['relationship'] == 'INVOCATES' and e['verificationStatus'] == 'VERIFIED' for e in d['edges'])" \
  "no verified INVOCATES edge was found. Either the engine has stopped recovering \
cross-contract calls, or the target contract has not called another contract within the \
lookback window. Check the evidence with --format markdown, and if the target has gone \
quiet widen --lookback or name a busier contract in scripts/live-target.env."
ok "edges found: $(python3 -c "import json;print(len(json.load(open('${dependencies_doc}'))['edges']))")"

step "graph (DOT)"
"${bin}" graph --contract "${contract}" --network "${network}" \
  --depth 2 --format dot --output "${workdir}/graph.dot" "${rpc_args[@]}"
if ! grep -q 'digraph' "${workdir}/graph.dot"; then
  echo "the graph export is not a DOT digraph:" >&2
  cat "${workdir}/graph.dot" >&2
  exit 1
fi
ok "graph written"

step "provenance"
"${bin}" provenance --contract "${contract}" --network "${network}" \
  --format json --output "${workdir}/provenance.json" "${rpc_args[@]}"
check "${workdir}/provenance.json" \
  "d['boundary']['ledger'] > 0" \
  "the provenance record reports no observation boundary"

step "verify"
verify_doc="${workdir}/verify.json"
"${bin}" verify --contract "${contract}" --network "${network}" \
  --format json --output "${verify_doc}" "${rpc_args[@]}"
check "${verify_doc}" \
  "d['status'] in ('VERIFIED','PARTIALLY_VERIFIED','UNVERIFIED','CONFLICTING','UNKNOWN')" \
  "the verification status is outside the specification's taxonomy"
check "${verify_doc}" \
  "d.get('reason')" \
  "a verification status was reported with no reason, which is a verdict without a warrant"
ok "verification status: $(python3 -c "import json;print(json.load(open('${verify_doc}'))['status'])")"

step "snapshot create"
# `--scan-events` is passed here for the same reason it is passed to `dependencies`:
# without it the snapshot is a record of a contract whose dependencies were never looked
# for, and the snapshot is the artifact everything downstream reads.
"${bin}" snapshot create --contract "${contract}" --network "${network}" \
  --depth 2 --scan-events "${lookback_args[@]}" --output "${workdir}" "${rpc_args[@]}"

# Found by its own naming rather than by "the newest json", because the scratch directory
# already holds the other commands' documents and picking whichever happened to be newest
# would make this step assert the wrong file.
snapshot_file="$(find "${workdir}" -maxdepth 1 -name 'snapshot-*.json' | sort | tail -n 1)"
if [ -z "${snapshot_file}" ]; then
  echo "test-testnet.sh: the snapshot command wrote no file, which is a defect." >&2
  exit 1
fi
check "${snapshot_file}" \
  "d.get('contentDigest', {}).get('value') and d['boundary']['ledger'] > 0" \
  "the snapshot has no content digest or no observation boundary, so it is not a snapshot"
# The snapshot must hold what the analysis found. A snapshot is the artifact every later
# step reads - the diff, the exports, the impact analysis - so one that captured nothing
# while the dependency command found edges would mean the two commands disagree about the
# same contract, and every result built on the snapshot would be built on the empty one.
check "${snapshot_file}" \
  "len(d['graph']['edges']) > 0" \
  "the snapshot holds no dependency edge even though the dependency analysis found some, \
so the snapshot describes a different contract state than the one that was analysed"
ok "snapshot written with content digest and $(python3 -c "import json,sys;print(len(json.load(open('${snapshot_file}'))['graph']['edges']))") edge(s)"

step "snapshot show"
"${bin}" snapshot show --input "${snapshot_file}"

step "report (Markdown and JUnit)"
"${bin}" report --contract "${contract}" --network "${network}" \
  --format markdown --output "${workdir}/report.md" "${rpc_args[@]}"
"${bin}" report --contract "${contract}" --network "${network}" \
  --depth 2 --format junit --output "${workdir}/report.xml" "${rpc_args[@]}"
for produced in "${workdir}/report.md" "${workdir}/report.xml"; do
  if [ ! -s "${produced}" ]; then
    echo "test-testnet.sh: ${produced} is empty, so the report rendered nothing." >&2
    exit 1
  fi
done
ok "reports written"

step "export (GraphML and YAML)"
"${bin}" export --input "${snapshot_file}" --format graphml \
  --output "${workdir}/graph.graphml"
"${bin}" export --input "${snapshot_file}" --format yaml --output "${workdir}/snapshot.yaml"
if ! grep -q 'graphml' "${workdir}/graph.graphml"; then
  echo "the GraphML export is not a GraphML document:" >&2
  head -5 "${workdir}/graph.graphml" >&2
  exit 1
fi
if [ ! -s "${workdir}/snapshot.yaml" ]; then
  echo "the YAML export is empty." >&2
  exit 1
fi
ok "exports written"

printf '\n\033[1;32mSmoke test passed.\033[0m The analysis was asserted against the chain.\n'
if [ "${keep_workdir}" = true ]; then
  echo "Artifacts are in ${workdir}."
else
  echo "Artifacts were written to a temporary directory and removed."
fi
echo "What was not asserted: ledger numbers, transaction hashes, event counts and the"
echo "identity of any dependency, because a live network changes all of them."
