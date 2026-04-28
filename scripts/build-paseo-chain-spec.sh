#!/bin/bash
# Build runtime and generate chain spec for zombienet
set -e

cd "$(dirname "$0")/.."

# Clean up any existing chain spec
rm -f chain_spec.json

# Build the runtime
cargo build --release -p storage-paseo-runtime >&2

# Generate chain spec using chain-spec-builder with local_testnet preset
.bin/chain-spec-builder create \
  -n "Paseo Web3 Storage Parachain" \
  -i "storage-paseo" \
  -t local \
  -p 1502 \
  -c westend-local \
  -r target/release/wbuild/storage-paseo-runtime/storage_paseo_runtime.compact.compressed.wasm \
  named-preset local_testnet

# Output the generated chain spec and clean up
cat chain_spec.json
rm chain_spec.json