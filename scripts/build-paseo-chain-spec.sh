#!/bin/bash
# Build runtime and generate chain spec for zombienet
set -eo pipefail

cd "$(dirname "$0")/.."

# Clean up any existing chain spec
rm -f chain_spec.json

# Ask cargo where it puts artifacts; a hardcoded target/ serves a stale wasm
# when CARGO_TARGET_DIR or build.target-dir redirects the build.
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
WASM="$TARGET_DIR/release/wbuild/storage-paseo-runtime/storage_paseo_runtime.compact.compressed.wasm"

# In CI the build job uploads this exact wasm and every test job downloads it
# into target/release before calling this script, so rebuilding it here is pure
# waste (~160s per job, on the critical path). Reuse the downloaded artifact when
# running under CI (GitHub sets CI=true). Locally we always rebuild so that
# runtime edits are never silently served from a stale wasm; set REBUILD_RUNTIME=1
# to force a rebuild in CI too.
if [ -f "$WASM" ] && [ -n "${CI:-}" ] && [ "${REBUILD_RUNTIME:-0}" != "1" ]; then
  echo "Reusing prebuilt runtime wasm (CI artifact): $WASM" >&2
else
  cargo build --release -p storage-paseo-runtime >&2
fi

# Generate chain spec using chain-spec-builder with local_testnet preset
.bin/chain-spec-builder create \
  -n "Paseo Web3 Storage Parachain" \
  -i "storage-paseo" \
  -t local \
  -p 1600 \
  -c westend-local \
  -r "$WASM" \
  named-preset local_testnet

# Output the generated chain spec and clean up
cat chain_spec.json
rm chain_spec.json