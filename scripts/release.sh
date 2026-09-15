#!/usr/bin/env bash
#
# Validate a release, and package it when asked.
#
# A release is only correct if the versions in the manifests, the CHANGELOG and the
# specification compatibility constants agree, and if every crate packages cleanly.
# Those are exactly the things that go wrong quietly: a sibling left at the previous
# version resolves for a local build and breaks for a consumer, and a changelog entry
# that names a version the manifest does not is a changelog nobody can trust.
#
# Publishing is opt-in. `--check-only` performs every validation and no publication;
# `--publish` additionally runs `cargo publish` in dependency order. This script never
# pushes a git tag and never pushes a commit - releasing is the caller's decision, and
# a script that pushed would be a script nobody could safely run to check a release.
#
# Usage:
#   scripts/release.sh --check-only
#   scripts/release.sh --publish [--dry-run]

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
cd "${repo_root}"

mode=""
dry_run=""
while [ $# -gt 0 ]; do
  case "$1" in
    --check-only) mode="check"; shift ;;
    --publish)    mode="publish"; shift ;;
    --dry-run)    dry_run="--dry-run"; shift ;;
    -h|--help)
      sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "release.sh: unrecognised argument $1" >&2
      exit 2
      ;;
  esac
done

if [ -z "${mode}" ]; then
  echo "release.sh: choose --check-only or --publish" >&2
  exit 2
fi

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
if [ -z "${version}" ]; then
  echo "release.sh: could not read the workspace version from Cargo.toml" >&2
  exit 1
fi
echo "workspace version: ${version}"

step "every crate declares the workspace version"
# A crate at a different version is not necessarily wrong, but it is almost always a
# mistake, and it is the mistake that breaks a release silently: the dependency
# requirement in a sibling manifest stops matching what is published.
bad=""
while IFS= read -r manifest; do
  # `version.workspace = true` is the intended form; anything else is a literal and
  # is compared against the workspace version.
  if ! grep -q '^version\.workspace = true' "${manifest}"; then
    own="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "${manifest}" | head -1)"
    if [ "${own}" != "${version}" ]; then
      bad="${bad}
  ${manifest}: ${own:-<unset>}"
    fi
  fi
done < <(find crates -maxdepth 2 -name Cargo.toml | sort)

if [ -n "${bad}" ]; then
  echo "release.sh: these crates do not declare version ${version}:${bad}" >&2
  exit 1
fi
echo "all crates agree"

step "the engine implements the specification version the workspace states"
engine_spec="$(sed -n 's/^pub const SUPPORTED_SPEC_VERSION: &str = "\(.*\)";/\1/p' \
  crates/amasario-core/src/engine.rs | head -1)"
if [ "${engine_spec}" != "${version}" ]; then
  echo "release.sh: amasario-core supports specification ${engine_spec} but the" >&2
  echo "workspace version is ${version}. A release whose version and supported" >&2
  echo "specification version disagree cannot be described accurately." >&2
  exit 1
fi
echo "engine supports specification ${engine_spec}"

step "the CHANGELOG has an entry for ${version}"
if ! grep -q "\[${version}\]" CHANGELOG.md && ! grep -q "## ${version}" CHANGELOG.md; then
  echo "release.sh: CHANGELOG.md has no entry for ${version}" >&2
  exit 1
fi
echo "changelog entry present"

step "every crate packages"
# `cargo package --list` resolves the manifest, applies include/exclude, and reports a
# file that would be missing from the published archive - the failure a consumer hits
# and a local build never does.
while IFS= read -r manifest; do
  crate_dir="$(dirname "${manifest}")"
  echo "-- ${crate_dir}"
  (cd "${crate_dir}" && cargo package --list --allow-dirty --quiet >/dev/null)
done < <(find crates -maxdepth 2 -name Cargo.toml | sort)

if [ "${mode}" = "check" ]; then
  printf '\n\033[1;32mRelease validation passed.\033[0m No crate was published.\n'
  exit 0
fi

step "publishing${dry_run:+ (dry run)}"
# Dependency order. `amasario-core` first because everything else depends on it, then
# the layers above it. A crate published before its dependency exists on the registry
# cannot be resolved by the consumers who fetch it.
order=(
  amasario-core
  amasario-network
  amasario-contract
  amasario-dependency
  amasario-evidence
  amasario-provenance
  amasario-graph
  amasario-impact
  amasario-snapshot
  amasario-report
  amasario-export
  amasario-cli
)

for crate in "${order[@]}"; do
  echo "-- ${crate}"
  (cd "crates/${crate}" && cargo publish --locked ${dry_run} --allow-dirty)
done

printf '\n\033[1;32mPublished.\033[0m Tag the release from the caller side when the registry has propagated.\n'
