#!/usr/bin/env bash
# Compile the Photos contract to PolkaVM bytecode and emit a single
# `src/contract/Photos.json` ({ abi, bin }) that both the deploy script and the
# UI import directly.
#
# Requirements (same toolchain as examples/contracts — see that README for
# install + version pins):
#   - solc on PATH
#   - resolc on PATH (Parity's Solidity-to-PolkaVM frontend)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$APP_DIR/contracts"      # .sol sources live here
BUILD_DIR="$CONTRACTS_DIR/build"        # .gitignored intermediate (combined.json)
OUT="$APP_DIR/src/contract/Photos.json" # tracked artifact the app imports

for bin in solc resolc; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        echo "error: $bin not found on PATH — see examples/contracts/README.md" >&2
        exit 1
    fi
done

mkdir -p "$BUILD_DIR" "$APP_DIR/src/contract"
cd "$CONTRACTS_DIR"

# Pass the interface explicitly so its ABI is available too; `Photos.sol:Photos`
# is the entry the extractor pulls out.
resolc --combined-json abi,bin -O3 --overwrite -o "$BUILD_DIR" \
    Photos.sol IDriveRegistry.sol

node "$SCRIPT_DIR/extract.mjs" "$BUILD_DIR/combined.json" "Photos.sol:Photos" "$OUT"
echo "Wrote $OUT"

# Regenerate the tracked, viem-typed ABI the UI imports from the compiled artifact.
ABI_TS="$APP_DIR/src/contract/photos-abi.ts"
node "$SCRIPT_DIR/gen-abi-ts.mjs" "$OUT" "$ABI_TS"
echo "Wrote $ABI_TS"
