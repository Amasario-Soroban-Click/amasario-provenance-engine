#!/usr/bin/env bash
#
# Builds the first-party reference contract to WebAssembly, in the order the call
# graph requires, and stages the results as committed fixtures.
#
# # Why two phases
#
# `caller/src/lib.rs` uses `contractimport!(file = "wasm/reference_callee.wasm")`, which
# reads the callee's *compiled* module at compile time in order to generate a typed
# client from its specification. The callee must therefore be built before the caller,
# which is why the two halves are separate workspaces rather than members of one.
#
# # Usage
#
#   scripts/build-reference-contract.sh            # build and stage
#
# The staged modules land in `fixtures/reference/`, where the integration suite reads
# them, and the printed digests are the ones recorded in `fixtures/reference/README.md`.
# This script is run by `.github/workflows/reference.yml` so that the fixtures cannot
# drift from their source unnoticed.

set -euo pipefail

# `wasm32v1-none`, not `wasm32-unknown-unknown`. From Rust 1.82 the SDK's build script
# refuses the older target outright, because that target enables `reference-types` and
# `multi-value`, which the Soroban environment does not support. `wasm32v1-none`
# requires Rust 1.84 or newer; the toolchain is pinned in `rust-toolchain.toml`.
readonly TARGET="wasm32v1-none"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly root

readonly callee_dir="$root/reference-contract/callee"
readonly caller_dir="$root/reference-contract/caller"
readonly staging="$root/fixtures/reference"

# Reproducibility, and the reason this is computed here rather than declared in a
# `.cargo/config.toml`.
#
# `soroban-sdk` has panic sites in its own `env.rs`, and a panic message carries the
# `file!()` of the file it was written in. For a dependency that is the absolute path of
# the registry directory the crate was unpacked from, so two machines with `CARGO_HOME`
# in different places produced different bytes from identical source: the first run of
# `reference.yml` built a 9331-byte caller where the committed fixture was 9335, and the
# whole difference was four bytes of `/home/runner` against `/home/codespace` inside one
# string in the `name` section. A fixture whose digest moves with the builder's home
# directory cannot be asserted against anything.
#
# Remapping both prefixes to fixed tokens makes the module a function of the source
# rather than of the machine that compiled it, which is what lets `reference.yml` require
# the committed modules to be what the source builds. Neither path can be spelled in a
# config file, because both depend on where cargo and the checkout happen to live on the
# machine doing the build - which is exactly why they are computed from the environment
# here, and why any pre-existing `RUSTFLAGS` is preserved rather than replaced.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
readonly cargo_home
export RUSTFLAGS="--remap-path-prefix=$cargo_home=/cargo \
--remap-path-prefix=$root=/amasario${RUSTFLAGS:+ $RUSTFLAGS}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to build the reference contract" >&2
  exit 1
fi

if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "the $TARGET target is required: rustup target add $TARGET" >&2
  exit 1
fi

# Phase one: the callee, whose module the caller imports.
echo "building reference-callee ($TARGET, release)"
cargo build --manifest-path "$callee_dir/Cargo.toml" --target "$TARGET" --release

readonly callee_wasm="$callee_dir/target/$TARGET/release/reference_callee.wasm"
if [ ! -f "$callee_wasm" ]; then
  echo "expected the callee module at $callee_wasm, but it is not there" >&2
  exit 1
fi

# Phase two: the caller, which reads the callee module staged just above.
mkdir -p "$caller_dir/wasm"
cp "$callee_wasm" "$caller_dir/wasm/reference_callee.wasm"

echo "building reference-caller ($TARGET, release)"
cargo build --manifest-path "$caller_dir/Cargo.toml" --target "$TARGET" --release

readonly caller_wasm="$caller_dir/target/$TARGET/release/reference_caller.wasm"
if [ ! -f "$caller_wasm" ]; then
  echo "expected the caller module at $caller_wasm, but it is not there" >&2
  exit 1
fi

# Stage both as fixtures, named for the contract they are rather than the crate, since
# the fixture name is what the provenance record refers to.
mkdir -p "$staging"
cp "$callee_wasm" "$staging/reference-callee.wasm"
cp "$caller_wasm" "$staging/reference-caller.wasm"

echo
echo "staged modules and their digests:"
for module in "$staging/reference-callee.wasm" "$staging/reference-caller.wasm"; do
  printf '  %-28s %s  %s bytes\n' \
    "$(basename "$module")" \
    "$(sha256sum "$module" | cut -d' ' -f1)" \
    "$(wc -c < "$module" | tr -d ' ')"
done
