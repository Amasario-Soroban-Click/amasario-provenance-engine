#!/usr/bin/env bash
#
# Install the `amasario` binary from this checkout.
#
# Builds from source with the locked dependency set, so an installed binary
# corresponds to the versions this repository pinned and tested rather than to
# whatever the registry resolves to on the day it runs.
#
# Usage:
#   scripts/install.sh [--root <dir>] [--force]
#
# Everything after the recognised options is passed through to `cargo install`,
# so `scripts/install.sh --root ~/.local --force` and
# `scripts/install.sh -- --debug` both work.

set -euo pipefail

usage() {
  cat <<'EOF'
Install the amasario binary from this checkout.

Usage:
  scripts/install.sh [--root <dir>] [--force] [-- <cargo install flags>]

Options:
  --root <dir>   Install into <dir>/bin instead of the cargo default (~/.cargo/bin).
  --force        Reinstall even when the binary is already present.
  -h, --help     Print this message.
EOF
}

root=""
force=""
passthrough=()

while [ $# -gt 0 ]; do
  case "$1" in
    --root)
      if [ $# -lt 2 ]; then
        echo "install.sh: --root needs a directory" >&2
        exit 2
      fi
      root="$2"
      shift 2
      ;;
    --force)
      force="--force"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      passthrough=("$@")
      break
      ;;
    *)
      passthrough+=("$1")
      shift
      ;;
  esac
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "install.sh: cargo was not found on PATH." >&2
  echo "Install Rust from https://rustup.rs and try again." >&2
  exit 1
fi

# The toolchain is pinned in rust-toolchain.toml, so an older compiler fails with a
# version error rather than a confusing type error. Report it here so the reason is
# named before the build starts.
required="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "${repo_root}/rust-toolchain.toml")"
if [ -n "${required}" ] && ! rustup toolchain list 2>/dev/null | grep -q "${required}"; then
  echo "install.sh: toolchain ${required} is not installed; fetching it."
  rustup toolchain install "${required}"
fi

install_args=(--path "${repo_root}/crates/amasario-cli" --locked)
if [ -n "${root}" ]; then
  install_args+=(--root "${root}")
fi
if [ -n "${force}" ]; then
  install_args+=("${force}")
fi
if [ ${#passthrough[@]} -gt 0 ]; then
  install_args+=("${passthrough[@]}")
fi

echo "==> cargo install ${install_args[*]}"
cargo install "${install_args[@]}"

# Where the binary landed, so the caller does not have to guess.
bindir="${root:+${root}/bin}"
bindir="${bindir:-${CARGO_HOME:-$HOME/.cargo}/bin}"
echo
echo "Installed. If '${bindir}' is not on your PATH, add it, then run:"
echo "  amasario --help"
