#!/usr/bin/env bash
#
# Validate every profile against a checkout of the normative specification.
#
# A profile names terms the specification owns: taxonomy files, relationship ids,
# network types, entity kinds. If one of those names drifts on either side, the
# profile silently stops describing what it claims to, and nothing notices until a
# consumer acts on a term that no longer means what it did. This check closes that
# gap by comparing the profile against the specification itself.
#
# It also checks the one protocol fact a profile restates: a network's id is the
# SHA-256 of its passphrase. A transcribed passphrase is exactly the kind of error
# that would otherwise survive review, because it looks right.
#
# Usage:
#   scripts/validate-profile.sh [profile.yaml ...]
#
# Environment:
#   AMASARIO_SPEC_DIR   A checkout of amasario-provenance-spec. Required.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

spec_dir="${AMASARIO_SPEC_DIR:-}"
if [ -z "${spec_dir}" ]; then
  for candidate in "${repo_root}/.amasario-spec" "${repo_root}/../amasario-provenance-spec"; do
    if [ -d "${candidate}" ]; then
      spec_dir="${candidate}"
      break
    fi
  done
fi

if [ -z "${spec_dir}" ] || [ ! -d "${spec_dir}" ]; then
  echo "validate-profile.sh: no specification checkout found." >&2
  echo "Set AMASARIO_SPEC_DIR to a checkout of amasario-provenance-spec." >&2
  exit 2
fi

if [ ! -d "${spec_dir}/taxonomies" ]; then
  echo "validate-profile.sh: ${spec_dir} does not look like the specification (no taxonomies/)." >&2
  exit 2
fi

if [ $# -gt 0 ]; then
  profiles=("$@")
else
  shopt -s nullglob
  profiles=("${repo_root}"/profiles/*.yaml)
  shopt -u nullglob
fi

if [ ${#profiles[@]} -eq 0 ]; then
  echo "validate-profile.sh: no profiles to validate." >&2
  exit 1
fi

# PyYAML is used rather than a hand-rolled parser because the profile is YAML with
# nesting and multi-line scalars, and a regular expression that handled today's file
# would silently mis-read tomorrow's. The dependency is bootstrapped rather than
# assumed so the script works on a bare runner.
if ! python3 -c 'import yaml' >/dev/null 2>&1; then
  echo "validate-profile.sh: installing PyYAML into the user site-packages."
  python3 -m pip install --user --quiet --disable-pip-version-check pyyaml
fi

python3 - "${spec_dir}" "${profiles[@]}" <<'PY'
"""Validate Amasario profiles against the normative specification."""

import hashlib
import os
import sys

import yaml

spec_dir = sys.argv[1]
profile_paths = sys.argv[2:]

problems: list[str] = []


def fail(message: str) -> None:
    problems.append(message)


def load(path: str):
    with open(path, encoding="utf-8") as handle:
        return yaml.safe_load(handle)


def taxonomy_ids(filename: str) -> set[str]:
    """The ids declared by a taxonomy, or None when the file is absent."""
    path = os.path.join(spec_dir, filename)
    if not os.path.exists(path):
        return None
    document = load(path)
    return {term["id"] for term in document.get("terms", [])}


for profile_path in profile_paths:
    name = os.path.relpath(profile_path)
    profile = load(profile_path)

    # -- the profiles are against the right specification ----------------------
    spec_block = profile.get("specification", {})
    if spec_block.get("specVersion") != profile.get("specVersion"):
        fail(
            f"{name}: specification.specVersion "
            f"{spec_block.get('specVersion')!r} does not match the profile's own "
            f"specVersion {profile.get('specVersion')!r}"
        )

    # -- every taxonomy it names exists ---------------------------------------
    for taxonomy in spec_block.get("taxonomies", []):
        if not os.path.exists(os.path.join(spec_dir, taxonomy)):
            fail(f"{name}: names a taxonomy that does not exist: {taxonomy}")

    # -- every relationship is a term of the relationship taxonomy ------------
    relationship_terms = taxonomy_ids("taxonomies/relationship-types.yaml")
    if relationship_terms is None:
        fail("taxonomies/relationship-types.yaml is missing from the specification")
        relationship_terms = set()
    for relationship in profile.get("relationships", []):
        if relationship["id"] not in relationship_terms:
            fail(
                f"{name}: relationship {relationship['id']!r} is not a term of "
                f"taxonomies/relationship-types.yaml"
            )

    # -- every network type is a term of the network taxonomy ------------------
    network_terms = taxonomy_ids("taxonomies/network-types.yaml")
    if network_terms is None:
        fail("taxonomies/network-types.yaml is missing from the specification")
        network_terms = set()
    for network in profile.get("networks", []):
        if network["type"] not in network_terms:
            fail(
                f"{name}: network {network['name']!r} has type {network['type']!r}, "
                f"which is not a term of taxonomies/network-types.yaml"
            )

        # -- the network id is the SHA-256 of the passphrase ------------------
        # The protocol rule, and the check that catches a transcribed passphrase.
        derived = hashlib.sha256(network["passphrase"].encode("utf-8")).hexdigest()
        if derived != network["networkId"]:
            fail(
                f"{name}: network {network['name']!r} records network id "
                f"{network['networkId']!r} but the SHA-256 of its passphrase is "
                f"{derived!r}"
            )

        # -- a network with no endpoint says why ------------------------------
        if network.get("rpc") is None and not network.get("requiresExplicitRpcEndpoint"):
            fail(
                f"{name}: network {network['name']!r} has no RPC endpoint and is not "
                f"marked requiresExplicitRpcEndpoint, so the reason no default exists "
                f"is lost"
            )

if problems:
    print("profile validation FAILED", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

print(f"profile validation passed ({len(profile_paths)} profile(s))")
PY
