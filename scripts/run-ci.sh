#!/usr/bin/env bash
#
# Run the same checks CI runs, locally.
#
# This mirrors .github/workflows/ci.yml rather than approximating it: the value of a
# local CI script is that it catches what the remote job would catch, and a script
# that runs a subset is one a contributor learns to distrust. Every step CI performs
# is performed here, and the specification-conformance step is skipped with a visible
# reason when no checkout of the specification is available rather than passing
# silently.
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

step "release validation"
"${script_dir}/release.sh" --check-only

printf '\n\033[1;32mAll checks passed.\033[0m\n'
