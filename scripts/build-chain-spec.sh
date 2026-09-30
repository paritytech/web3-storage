#!/bin/bash
#
# Build runtime and emit a parachain chain spec to stdout.
#
# Used by `just generate-chain-spec`, which redirects the output to chain-spec.json.
# Standalone usage: ./scripts/build-chain-spec.sh > chain-spec.json
#
# Requires: chain-spec-builder downloaded to .bin/ (see `just download-binaries`).
# Para ID: 4000. Preset: local_testnet. Relay: westend-local.
set -eo pipefail

cd "$(dirname "$0")/.."

# Clean up any existing chain spec
rm -f chain_spec.json

# Build the runtime
cargo build --release -p storage-parachain-runtime >&2

# Ask cargo where it puts artifacts; a hardcoded target/ serves a stale wasm
# when CARGO_TARGET_DIR or build.target-dir redirects the build.
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)

# Generate chain spec using chain-spec-builder with local_testnet preset
.bin/chain-spec-builder create \
  -n "Web3 Storage Local" \
  -i "storage-local" \
  -t local \
  -p 4000 \
  -c westend-local \
  -r "$TARGET_DIR/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm" \
  named-preset local_testnet

# Output the generated chain spec and clean up
cat chain_spec.json
rm chain_spec.json
