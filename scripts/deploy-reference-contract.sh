#!/usr/bin/env bash
#
# Deploys the first-party reference contract to a live Stellar network.
#
# # Why this is a script and not a paragraph of instructions
#
# The reference contract is the one artefact this repository deploys, and a deployment
# that only ever happened on a maintainer's machine is a deployment nobody can check. The
# contract IDs in `docs/testnet.md` mean something only if a reader can reproduce the
# deployment the IDs came from, so the commands are committed rather than remembered.
#
# # What it deploys, and why the fixtures rather than the build output
#
# The modules under `fixtures/reference/`, which are the bytes `reference.yml` asserts are
# what `reference-contract/` builds. Deploying from `target/` would deploy whatever the
# last build left there, and the entire point of this contract is that its deployed module
# is the one whose digest is committed. So the script re-checks that after each
# deployment, using this repository's own `amasario inspect`: it reads the wasm hash the
# chain reports and stops if it is not the fixture's sha256. A deployment this script
# reports as successful is one where the deployed bytes and the committed fixture are the
# same bytes, which is a claim about the world rather than about a local directory.
#
# # No key is handled here
#
# The identity is created and funded by the `stellar` CLI, which keeps it in its own
# keystore outside this repository. This script never reads, prints or copies a secret; it
# names an identity and the CLI signs with it. Nothing in the engine touches a key, and
# this script is a maintainer tool that is not part of the engine's workspace.
#
# # Usage
#
#   scripts/deploy-reference-contract.sh                       # testnet, default identity
#   scripts/deploy-reference-contract.sh --network futurenet
#   scripts/deploy-reference-contract.sh --identity some-name
#   scripts/deploy-reference-contract.sh --invoke              # also send the two calls
#
# Progress goes to standard error and the final record to standard output, so the record
# can be captured without the commentary:
#
#   scripts/deploy-reference-contract.sh --invoke 2>/dev/null | jq .callee.contractId
#
# Requires the `stellar` CLI, `jq` and a built `amasario` binary on `PATH` or at
# `target/{release,debug}/amasario`. The identity has to exist and be funded; the script
# says how to make one rather than making it, because creating a funded account is a
# decision about whose account it is.
#
# `--invoke` sends two real transactions: the read-only cross-contract call, and the
# authenticated call that reaches the callee's `require_auth` one frame down. The second is
# the one that matters for analysis, and the reason is worth stating: the engine's evidence
# for a cross-contract edge is an *event*, and only the callee emits one here. So the
# authenticated call is what gives the deployed pair an edge the engine can observe -
# `amasario discover --contract <callee> --scan-events` then reports the caller entering it
# - and without it a fresh deployment is two contracts with no history between them.
#
# `--auth-mode non-root` on that call is not optional. Without it the callee's
# `require_auth` fails with `Error(Auth, InvalidAction)`: the account never appears in the
# root invocation, so authorization one frame down is not recorded. The caller's own test in
# `reference-contract/caller/src/lib.rs` documents the same shape, and the SDK's second
# `mock_all_auths` variant exists for exactly this case.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly root

network="${AMASARIO_DEPLOY_NETWORK:-testnet}"
identity="${AMASARIO_DEPLOY_IDENTITY:-amasario-reference}"
send_calls=0

while [ $# -gt 0 ]; do
  case "$1" in
    --network)
      network="${2:?--network needs a value}"
      shift 2
      ;;
    --identity)
      identity="${2:?--identity needs a value}"
      shift 2
      ;;
    --invoke)
      send_calls=1
      shift
      ;;
    --help | -h)
      # A heredoc rather than a `sed` range over this file's own header. The header is
      # extracted with `sed -n '/^# # Usage/,/^#$/p'` in the obvious version, and that
      # range ends on the first line that is a bare `#` - which is the blank separator
      # *inside* the usage block, so it printed the heading and nothing else.
      cat <<'USAGE'
Deploys the first-party reference contract to a live Stellar network.

Usage:
  scripts/deploy-reference-contract.sh [--network <name>] [--identity <name>] [--invoke]

  --network <name>    the Stellar network to deploy to (default: testnet)
  --identity <name>   the stellar CLI keystore identity to sign with; it must exist and
                      be funded, and the script says how to make one rather than making it
  --invoke            also send the two cross-contract calls, which is what gives the
                      deployed pair an edge the engine can observe

Environment:
  AMASARIO_DEPLOY_NETWORK    the default for --network
  AMASARIO_DEPLOY_IDENTITY   the default for --identity
  AMASARIO_BIN               the amasario binary used to check what was deployed

The deployed module's on-chain wasm hash is compared against the committed fixture's
sha256, and the script stops if they differ. Progress goes to standard error and the
record to standard output.
USAGE
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      echo "try --help" >&2
      exit 2
      ;;
  esac
done

for tool in stellar jq sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "${tool} is required and is not on PATH" >&2
    exit 1
  fi
done

# This repository's own binary, because the verification below is the engine reading the
# chain rather than a second implementation of the same check. `AMASARIO_BIN` overrides it
# so a specific build can be named.
engine="${AMASARIO_BIN:-}"
if [ -z "$engine" ]; then
  for candidate in "$root/target/release/amasario" "$root/target/debug/amasario"; do
    if [ -x "$candidate" ]; then
      engine="$candidate"
      break
    fi
  done
fi

if [ -z "$engine" ] || [ ! -x "$engine" ]; then
  echo "no amasario binary found. Build one:" >&2
  echo "  cargo build --release -p amasario-cli" >&2
  echo "or point AMASARIO_BIN at an existing binary." >&2
  exit 1
fi

readonly callee_wasm="$root/fixtures/reference/reference-callee.wasm"
readonly caller_wasm="$root/fixtures/reference/reference-caller.wasm"

for module in "$callee_wasm" "$caller_wasm"; do
  if [ ! -f "$module" ]; then
    echo "the fixture ${module#"$root"/} is missing." >&2
    echo "Build the pair, which stages them: scripts/build-reference-contract.sh" >&2
    exit 1
  fi
done

# Captured rather than assumed, because the failure a reader hits first is a fresh machine
# with no identity, and the remedy is a command they have to be told rather than guess.
if ! address="$(stellar keys address "$identity" 2>/dev/null)"; then
  echo "no identity named ${identity} in the stellar CLI's keystore." >&2
  echo "Create and fund one, then run this again:" >&2
  echo "  stellar keys generate ${identity} --network ${network} --fund" >&2
  exit 1
fi

echo "network   ${network}" >&2
echo "identity  ${identity}" >&2
echo "deployer  ${address}" >&2
echo >&2

# Deploys one module and refuses to return an id it has not checked against the chain.
# Progress goes to stderr so the caller can capture the id from stdout; the whole function
# is written around that, and a diagnostic printed to stdout would end up inside the id.
deploy_and_verify() {
  local name="$1"
  local wasm="$2"
  local contract_id
  local expected
  local actual

  echo "deploying ${name}" >&2
  contract_id="$(stellar contract deploy \
    --wasm "$wasm" \
    --source "$identity" \
    --network "$network")"

  if ! printf '%s' "$contract_id" | grep -Eq '^C[A-Z2-7]{55}$'; then
    echo "the deploy did not return a contract id; it returned:" >&2
    echo "$contract_id" >&2
    exit 1
  fi

  expected="$(sha256sum "$wasm" | cut -d' ' -f1)"
  actual="$("$engine" inspect --contract "$contract_id" --network "$network" --format json \
    | jq -r '.identity.wasmHash.value')"

  echo "  contract  ${contract_id}" >&2
  echo "  fixture   ${expected}" >&2
  echo "  on chain  ${actual}" >&2

  if [ "$expected" != "$actual" ]; then
    echo >&2
    echo "the deployed module is not the committed fixture, so nothing this script could" >&2
    echo "record about ${name} would be worth recording. Either the fixture moved after it" >&2
    echo "was built, or the deployment was made from a different module." >&2
    exit 1
  fi

  echo "  the deployed bytes are the committed fixture" >&2
  echo >&2

  printf '%s' "$contract_id"
}

callee_id="$(deploy_and_verify "the callee (Ledger)" "$callee_wasm")"
caller_id="$(deploy_and_verify "the caller (Observer)" "$caller_wasm")"

if [ "$send_calls" -eq 1 ]; then
  echo "sending the read-only cross-contract call: caller -> callee" >&2
  stellar contract invoke \
    --id "$caller_id" \
    --source "$identity" \
    --network "$network" \
    --send=yes \
    -- observe --ledger "$callee_id" --account "$address" >&2
  echo >&2

  echo "sending the authenticated nested-authorization call: caller -> callee" >&2
  stellar contract invoke \
    --id "$caller_id" \
    --source "$identity" \
    --network "$network" \
    --send=yes \
    --auth-mode non-root \
    -- record_via --ledger "$callee_id" --account "$address" --amount 7 >&2
  echo >&2

  # The engine's own reading of what the two calls left behind. Reported rather than
  # asserted: the scan window is the node's recent history, so a run that finds no edge may
  # be looking at a stretch of chain the transaction has not landed in yet, and a script
  # that failed on that would be reporting its own timing rather than the deployment.
  echo "what the engine can see at the callee now:" >&2
  "$engine" discover --contract "$callee_id" --network "$network" --scan-events >&2 || true
fi

if [ "$send_calls" -eq 1 ]; then
  calls_boolean=true
else
  calls_boolean=false
fi

jq -n \
  --arg network "$network" \
  --arg identity "$identity" \
  --arg deployer "$address" \
  --arg callee "$callee_id" \
  --arg caller "$caller_id" \
  --arg callee_digest "$(sha256sum "$callee_wasm" | cut -d' ' -f1)" \
  --arg caller_digest "$(sha256sum "$caller_wasm" | cut -d' ' -f1)" \
  --argjson calls_sent "$calls_boolean" \
  '{
     network: $network,
     identity: $identity,
     deployer: $deployer,
     callee: {contractId: $callee, wasmDigest: $callee_digest},
     caller: {contractId: $caller, wasmDigest: $caller_digest},
     callsSent: $calls_sent
   }'
