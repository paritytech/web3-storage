#!/bin/bash
# Build runtime and generate chain spec for zombienet
set -e

cd "$(dirname "$0")/.."

# Clean up any existing chain spec
rm -f chain_spec.json

# Build the runtime
cargo build --release -p storage-parachain-runtime >&2

# Generate chain spec using chain-spec-builder with local_testnet preset
.bin/chain-spec-builder create \
  -n "Storage Local" \
  -i "storage-local" \
  -t local \
  -p 4000 \
  -c westend-local \
  -r target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm \
  named-preset local_testnet

# Output the generated chain spec and clean up
cat chain_spec.json
rm chain_spec.json
