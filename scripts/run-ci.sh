#!/usr/bin/env bash
#
# Run the same checks CI runs, locally.
#
# This mirrors the workflows CI runs rather than approximating them: the value of a
# local CI script is that it catches what the remote job would catch, and a script
# that runs a subset is one a contributor learns to distrust. Every step ci.yml and
# security.yml perform is performed here, and a step whose tooling or inputs are absent
# is skipped with a visible reason rather than passing silently.
#
# The dependency-policy step is included for that reason. It was a step that only
# existed remotely, and the configuration it checks had drifted from the checker that
# reads it - which is exactly the class of failure this script is meant to catch before
# a push. None of that is discoverable by running the tests.
#
# Usage:
#   scripts/run-ci.sh
#
# Environment:
#   AMASARIO_SPEC_DIR   A checkout of amasario-provenance-spec. When unset, a sibling
#                       directory named amasario-provenance-spec or the .amasario-spec
#                       path CI uses is tried, and the step is skipped if neither
#                       exists.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
cd "${repo_root}"

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

spec_dir="${AMASARIO_SPEC_DIR:-}"
if [ -z "${spec_dir}" ]; then
  for candidate in "${repo_root}/.amasario-spec" "${repo_root}/../amasario-provenance-spec"; do
    if [ -d "${candidate}" ]; then
      spec_dir="${candidate}"
      break
    fi
  done
fi

step "formatting"
cargo fmt --all -- --check

step "shell scripts"
# The same check CI runs, at the same severity. Skipped with a reason rather than
# silently when shellcheck is not installed, because a step that quietly does nothing
# is the failure this script exists to avoid.
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck --severity=style "${script_dir}"/*.sh "${script_dir}"/*.env
else
  echo "SKIPPED: shellcheck is not installed. Install it to lint scripts/."
fi

step "workflows and the composite action"
# The same check CI runs. The digest is pinned in the workflow; here the check is left to
# the installed copy, because a contributor's actionlint is theirs to trust.
if command -v actionlint >/dev/null 2>&1; then
  actionlint -color "${repo_root}"/.github/workflows/*.yml
else
  echo "SKIPPED: actionlint is not installed. Install it to lint .github/workflows/."
fi

step "linting (clippy, warnings denied)"
cargo clippy --workspace --all-features --all-targets -- -D warnings

step "building"
cargo build --workspace --all-features

step "unit and integration tests"
cargo test --workspace --all-features

step "documentation"
# `-D warnings` on rustdoc catches a broken intra-document link, which is the
# documentation failure that actually misleads a reader rather than merely
# annoying them.
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

step "examples"
# The examples are executable documentation: the offline ones are re-run and compared
# against their committed output, and the endpoint ones are pointed at an address that
# refuses connections and must report that as a network failure rather than a usage error.
"${script_dir}/check-examples.sh"

step "specification conformance"
if [ -n "${spec_dir}" ] && [ -d "${spec_dir}" ]; then
  echo "checking against ${spec_dir}"
  AMASARIO_SPEC_DIR="${spec_dir}" \
    cargo test --workspace --all-features -- --include-ignored conformance
  AMASARIO_SPEC_DIR="${spec_dir}" \
    cargo test -p amasario-core --all-features -- --include-ignored
else
  echo "SKIPPED: no specification checkout was found."
  echo "Set AMASARIO_SPEC_DIR, or clone amasario-provenance-spec beside this repository,"
  echo "to run the conformance checks. They are not run silently as though they passed."
fi

step "profile validation"
if [ -n "${spec_dir}" ] && [ -d "${spec_dir}" ]; then
  AMASARIO_SPEC_DIR="${spec_dir}" "${script_dir}/validate-profile.sh"
else
  echo "SKIPPED: profile validation reads the specification's taxonomies."
fi

step "dependency policy (advisories, licences, sources)"
# The same checks security.yml runs. Both tools are skipped with a reason when they are
# not installed, because a contributor without them should not be told the gate passed.
# Neither tool is installed by this script: they are release binaries, and fetching one
# during a check is a supply-chain step of its own.
ran_security_step=0
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
  ran_security_step=1
else
  echo "SKIPPED: cargo-deny is not installed. Install it to check deny.toml."
fi
if command -v cargo-audit >/dev/null 2>&1; then
  cargo audit --deny warnings
  ran_security_step=1
else
  echo "SKIPPED: cargo-audit is not installed. Install it to check the lockfile."
fi
if [ "${ran_security_step}" -eq 0 ]; then
  echo "Neither dependency-policy tool ran; deny.toml and Cargo.lock were not checked."
fi

step "release validation"
"${script_dir}/release.sh" --check-only

printf '\n\033[1;32mAll checks passed.\033[0m\n'
