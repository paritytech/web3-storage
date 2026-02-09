#!/bin/bash
# Build runtime and generate chain spec for zombienet
set -e

cd "$(dirname "$0")/.."

# Clean up any existing chain spec
rm -f chain_spec.json

WASM_PATH="target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm"

# Check if WASM exists, if not try to build (may fail with some Rust versions)
if [[ ! -f "$WASM_PATH" ]]; then
    echo "WASM not found, attempting to build..." >&2
    # Try building with SKIP_WASM_BUILD first to compile native code
    SKIP_WASM_BUILD=1 cargo build --release -p storage-parachain-runtime >&2 || true
    # Then try the actual WASM build
    cargo build --release -p storage-parachain-runtime >&2 || {
        echo "WASM build failed. If using Rust 1.88+, try extracting from existing chain-spec:" >&2
        echo "  cat chain-specs/storage-local.json | jq -r '.genesis.runtimeGenesis.code' | cut -c3- | xxd -r -p > $WASM_PATH" >&2
        exit 1
    }
fi

# Generate chain spec using chain-spec-builder with patched genesis
SCRIPT_DIR="$(dirname "$0")"
.bin/chain-spec-builder create \
  -n "Storage Local" \
  -i "storage-local" \
  -t local \
  -p 4000 \
  -c westend-local \
  -r "$WASM_PATH" \
  patch "$SCRIPT_DIR/genesis-patch.json"

# Output the generated chain spec and clean up
cat chain_spec.json
rm chain_spec.json
