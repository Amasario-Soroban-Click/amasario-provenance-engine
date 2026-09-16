#!/usr/bin/env bash
#
# Measure what each reference-contract entrypoint costs on a live network.
#
# Soroban has no gas. What it has is a metered resource budget, and the price of an
# invocation is the fee the network charges for the resources it actually used: CPU
# instructions, memory, ledger reads and writes, transaction size and events. The
# resource fee is charged partly upfront and partly refunded against what was really
# consumed, so "proposed" and "charged" are different numbers and the difference is the
# point - a transaction is billed for what it used, not for what it asked for.
#
# This is deliberately not `scripts/benchmark.sh`. That one runs the engine's Criterion
# benchmarks, which measure how long the analyser takes on this machine. This one measures
# what the *contracts* cost on Testnet, which is a property of chain rather than of the
# machine. Two different questions, two different scripts, and keeping them apart is why
# neither name is ambiguous.
#
# It sends real transactions, because that is the only way to observe what the network
# actually charges. `stellar` can report a simulated fee without sending, and that number
# is useful, but it is an estimate by the simulator rather than a receipt. Both are
# printed, and the column names say which is which.
#
# Usage:
#   scripts/benchmark-contracts.sh --identity <name> [--network testnet] [--json]
#
# Environment:
#   STELLAR_IDENTITY   The identity to sign with, if --identity is not given.
#   STELLAR_PATH       Prepended to PATH, so a locally installed `stellar` is found.
#
# The identity must exist in the Stellar CLI's keystore and be funded on the network. No
# key is read, written or printed by this script: it passes the identity's *name* to the
# CLI and lets the CLI do the signing, so nothing secret enters this process's arguments
# or its output.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
cd "${repo_root}"

if [ -n "${STELLAR_PATH:-}" ]; then
  PATH="${STELLAR_PATH}:${PATH}"
  export PATH
fi

network="testnet"
identity="${STELLAR_IDENTITY:-}"
json=false

while [ $# -gt 0 ]; do
  case "$1" in
    --identity) identity="${2:-}"; shift 2 ;;
    --network)  network="${2:-}"; shift 2 ;;
    --json)     json=true; shift ;;
    -h|--help)
      # Only the leading comment block, so the help cannot drift from the documentation
      # above it the way a hand-maintained usage string does.
      sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "benchmark-contracts.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

if ! command -v stellar >/dev/null 2>&1; then
  echo "benchmark-contracts.sh: the 'stellar' CLI is not on PATH." >&2
  echo "Install it, or set STELLAR_PATH to the directory holding it." >&2
  exit 2
fi

if [ -z "${identity}" ]; then
  echo "benchmark-contracts.sh: no identity given." >&2
  echo "Pass --identity <name> or set STELLAR_IDENTITY. The identity must be funded on" >&2
  echo "${network}; this script sends transactions, which cost a fee." >&2
  exit 2
fi

# The contract ids are read from the page that records the deployment rather than
# duplicated here. A second copy is a second thing to forget to update, and the failure
# would be a benchmark of contracts that are not the ones documented.
testnet_doc="docs/testnet.md"
if [ ! -f "${testnet_doc}" ]; then
  echo "benchmark-contracts.sh: ${testnet_doc} is missing, so the deployed ids cannot be read." >&2
  exit 2
fi

read_id() {
  local role="$1"
  # The row looks like: | callee — `Ledger` | `C...` | `sha256` |
  sed -n "s/^| *${role}.*| *\`\(C[A-Z2-7]\{55\}\)\`.*/\1/p" "${testnet_doc}" | head -1
}

callee_id="$(read_id callee)"
caller_id="$(read_id caller)"

if [ -z "${callee_id}" ] || [ -z "${caller_id}" ]; then
  echo "benchmark-contracts.sh: could not read both contract ids from ${testnet_doc}." >&2
  echo "callee='${callee_id}' caller='${caller_id}'" >&2
  exit 1
fi

account="$(stellar keys address "${identity}" 2>/dev/null || true)"
if [ -z "${account}" ]; then
  echo "benchmark-contracts.sh: no address for identity '${identity}'." >&2
  echo "Create it with 'stellar keys generate ${identity} --network ${network}'." >&2
  exit 2
fi

# The measured rows, tab separated as: label, proposed, charged, non-refundable,
# refunded. Kept as plain text until the end so `--json` and the markdown table are
# rendered from one measurement rather than from two runs that could disagree.
rows_file="$(mktemp)"
trap 'rm -f "${rows_file}"' EXIT

# One entrypoint. Sends the transaction, captures the cost block, and reduces it to the
# figures a reader needs: what the simulator proposed, what was charged, how much of that
# was non-refundable, and how much came back.
# $4 is a space-separated list of `invoke`-level flags, which have to precede the `--`
# that ends option parsing: `--auth-mode` is accepted by `invoke` and not by the
# entrypoint's own argument parser, so putting it after the entrypoint name is a usage
# error rather than a silently ignored flag.
measure() {
  local label="$1" contract="$2" entry="$3" invoke_flags="$4"
  shift 4

  echo "  measuring ${label}…" >&2
  local out status
  # `--cost` writes the fee breakdown to stderr and only when a transaction is built, so
  # this must send. A failure is reported rather than retried: a benchmark that silently
  # substituted an estimate for a receipt would be worse than one that stops.
  set +e
  # shellcheck disable=SC2086 # invoke_flags is deliberately word-split into separate flags
  out="$(stellar contract invoke \
    --id "${contract}" \
    --network "${network}" \
    --source "${identity}" \
    --send=yes --cost \
    ${invoke_flags} \
    -- "${entry}" "$@" 2>&1 1>/dev/null)"
  status=$?
  set -e

  if [ "${status}" -ne 0 ]; then
    echo "benchmark-contracts.sh: ${label} failed (exit ${status}):" >&2
    printf '%s\n' "${out}" >&2
    exit 1
  fi

  # The report is drawn with box characters, so they are removed before parsing rather
  # than matched around them - matching around them ties the parser to one CLI's table
  # width, and the table is a presentation choice the CLI is free to change.
  local flat
  flat="$(printf '%s\n' "${out}" | tr -d '│┌┐└┘├┤┬┴┼─' | tr -s ' ')"

  # The last match, not the first. The breakdown prints a per-component row before the
  # total, and `Inclusion Fee Charged: 100` also contains the substring `Fee Charged` -
  # so the first match is the inclusion fee, which is 100 stroops on every transaction
  # and would have made every entrypoint appear to cost the same. The total is the last
  # one printed, and it is the number a caller is actually billed.
  figure() {
    printf '%s\n' "${flat}" | grep -o "$1: [0-9][0-9]*" | tail -1 | grep -o '[0-9][0-9]*'
  }

  local proposed inclusion non_refundable refundable refunded charged
  proposed="$(figure "Fee Proposed")"
  inclusion="$(figure "Inclusion Fee Charged")"
  non_refundable="$(figure "Non-Refundable Charged")"
  refundable="$(figure "Refundable Charged")"
  refunded="$(figure "Refunded")"
  charged="$(figure "Fee Charged")"

  for value in "${proposed}" "${inclusion}" "${non_refundable}" "${refundable}" "${refunded}" "${charged}"; do
    if [ -z "${value}" ]; then
      echo "benchmark-contracts.sh: could not read the fee breakdown for ${label}." >&2
      echo "The CLI's output format may have changed. Raw output follows:" >&2
      printf '%s\n' "${out}" >&2
      exit 1
    fi
  done

  # The parser checks its own arithmetic rather than trusting its regexes. The total the
  # CLI reports must equal the inclusion fee plus both resource components; if a figure
  # was read from the wrong row, the sum will not agree and this stops instead of
  # publishing a table of plausible numbers that happen to be the wrong ones.
  if [ "$((inclusion + non_refundable + refundable))" -ne "${charged}" ]; then
    echo "benchmark-contracts.sh: the fee breakdown for ${label} does not add up." >&2
    echo "inclusion ${inclusion} + non-refundable ${non_refundable} + refundable" >&2
    echo "${refundable} = $((inclusion + non_refundable + refundable)), but the CLI" >&2
    echo "reports a total charged of ${charged}." >&2
    exit 1
  fi

  printf '%s\t%s\t%s\t%s\t%s\n' \
    "${label}" "${proposed}" "${charged}" "${non_refundable}" "${refunded}" >> "${rows_file}"
  echo "    proposed ${proposed}, charged ${charged} (inclusion ${inclusion}, non-refundable ${non_refundable}, refundable ${refundable}), refunded ${refunded}" >&2
}

echo "Benchmarking the reference contract on ${network} as $(printf '%.8s' "${account}")…" >&2
echo "  callee ${callee_id}" >&2
echo "  caller ${caller_id}" >&2
echo >&2

measure "callee.sequence" "${callee_id}" sequence "" --account "${account}"
measure "callee.record" "${callee_id}" record "" --account "${account}" --amount 1
measure "caller.observe" "${caller_id}" observe "" --ledger "${callee_id}" --account "${account}"
# The nested-authorization path, and the reason this row is in the table. The callee's
# `require_auth` is reached one frame below the invocation the caller makes, so the
# account never appears in the root invocation's authorization entries and the CLI needs
# `--auth-mode non-root` to build a transaction that can succeed. A caller that skipped
# this would benchmark a transaction that fails, and report its fee as the cost of the
# path it never took.
measure "caller.record_via" "${caller_id}" record_via "--auth-mode non-root" \
  --ledger "${callee_id}" --account "${account}" --amount 1

if [ "${json}" = true ]; then
  awk -F'\t' 'BEGIN { print "[" }
    { printf "%s  {\"entrypoint\": \"%s\", \"proposed\": %s, \"charged\": %s, \"non_refundable\": %s, \"refunded\": %s}",
             (NR > 1 ? ",\n" : ""), $1, $2, $3, $4, $5 }
    END { print "\n]" }' "${rows_file}"
  exit 0
fi

# The markdown table. `--` where a figure does not apply would be worse than a number:
# every one of these is a measured integer, and a reader comparing rows should see four
# numbers on every line or none.
printf '| Entrypoint | Fee proposed (stroops) | Fee charged (stroops) | Non-refundable | Refunded |\n'
printf '| --- | --- | --- | --- | --- |\n'
awk -F'\t' '{ printf "| `%s` | %s | %s | %s | %s |\n", $1, $2, $3, $4, $5 }' "${rows_file}"
