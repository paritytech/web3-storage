#!/usr/bin/env bash
# Compile the example Solidity contracts to PolkaVM bytecode.
#
# Requirements (see README.md for install + version pins):
#   - solc on PATH
#   - resolc on PATH (Parity's Solidity-to-PolkaVM frontend)
#
# Outputs land in ./build/ and are .gitignored.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/build"

for bin in solc resolc; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        echo "error: $bin not found on PATH — see examples/contracts/README.md" >&2
        exit 1
    fi
done

mkdir -p "$BUILD_DIR"
cd "$SCRIPT_DIR"

# `--combined-json abi,bin` emits one JSON file with both the ABI and the
# PolkaVM bytecode keyed by `<file>:<contract>`. Single artifact is easier
# for the e2e driver to load than per-contract .bin / .abi pairs.
#
# All three .sol files are passed explicitly so the interfaces' ABIs land in
# `combined.json` too — `sc-coverage.js` calls the precompiles directly using
# those ABIs, without an intermediate contract.
resolc --combined-json abi,bin -O3 --overwrite -o "$BUILD_DIR" \
    StorageMarketplace.sol SharedTeamDrive.sol \
    IWeb3Storage.sol IDriveRegistry.sol

echo "Built artifacts:"
ls -1 "$BUILD_DIR" | sed 's/^/  /'
