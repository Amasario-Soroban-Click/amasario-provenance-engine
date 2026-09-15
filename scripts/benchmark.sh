#!/usr/bin/env bash
#
# Run the benchmarks.
#
# The engine's benchmarks are Criterion benchmarks in `benches/`. They exist to make a
# performance regression visible when it is introduced rather than when a user reports
# an analysis that takes minutes, and to keep a claim about bounded traversal honest:
# a bound that is correct but quadratic is still a problem.
#
# Usage:
#   scripts/benchmark.sh [bench name ...]
#
# Environment:
#   CARGO_PROFILE   `bench` (default) or `release`.
#   BENCH_ARGS      Extra Criterion arguments, e.g. '--quick' or '--save-baseline main'.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
cd "${repo_root}"

if [ ! -d benches ] || [ -z "$(find benches -maxdepth 1 -name '*.rs' -print -quit)" ]; then
  echo "benchmark.sh: no benchmarks found in benches/." >&2
  echo "Nothing is run, because reporting success for a benchmark suite that does not" >&2
  echo "exist would be a false statement." >&2
  exit 1
fi

profile="${CARGO_PROFILE:-bench}"
# shellcheck disable=SC2086 # BENCH_ARGS is intentionally word-split.
bench_args=${BENCH_ARGS:-}

if [ "$#" -gt 0 ]; then
  for bench in "$@"; do
    echo "==> cargo bench --profile ${profile} --bench ${bench} -- ${bench_args}"
    # shellcheck disable=SC2086
    cargo bench --profile "${profile}" --bench "${bench}" -- ${bench_args}
  done
else
  echo "==> cargo bench --profile ${profile} -- ${bench_args}"
  # shellcheck disable=SC2086
  cargo bench --profile "${profile}" -- ${bench_args}
fi

echo
echo "Criterion wrote its report under target/criterion/. The baseline is the"
echo "comparison point for the next run; use BENCH_ARGS=--save-baseline <name> to"
echo "keep a named one."
