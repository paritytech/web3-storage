# Scalable Web3 Storage - Development Commands
#
# Install just:
#   cargo install just
# Or on macOS:
#   brew install just

# Polkadot SDK version (matches Cargo.toml tag)
polkadot_version := "polkadot-stable2512"

# Detect OS and architecture
os := `uname -s | tr '[:upper:]' '[:lower:]'`
arch := `uname -m`

# URL components
polkadot_sdk_base := "https://github.com/paritytech/polkadot-sdk/releases/download/" + polkadot_version + "/"
darwin_suffix := if os == "darwin" { "-aarch64-apple-darwin" } else { "" }
zombienet_asset := if os == "darwin" { if arch == "arm64" { "zombienet-macos-arm64" } else { "zombienet-macos-x64" } } else { "zombienet-linux-x64" }

# Provider port (override with: just PORT=3001 start-provider)
PORT := "3333"

# Default recipe
default:
    @just --list

# Build the project
build:
    cargo build --release

[private]
_download BIN URL:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .bin
    if [[ -x .bin/{{BIN}} ]]; then
        echo "{{BIN}} already exists in .bin/"
        exit 0
    fi
    echo "Downloading {{BIN}}..."
    curl -L -o .bin/{{BIN}} "{{URL}}"
    chmod +x .bin/{{BIN}}
    echo "{{BIN}} downloaded to .bin/{{BIN}}"

# Download all required binaries
[private]
download-binaries: download-polkadot download-polkadot-omni-node download-chain-spec-builder download-zombienet
    @echo "All binaries downloaded to .bin/"

[private]
download-polkadot: (_download "polkadot" polkadot_sdk_base + "polkadot" + darwin_suffix) (_download "polkadot-execute-worker" polkadot_sdk_base + "polkadot-execute-worker" + darwin_suffix) (_download "polkadot-prepare-worker" polkadot_sdk_base + "polkadot-prepare-worker" + darwin_suffix)

[private]
download-polkadot-omni-node: (_download "polkadot-omni-node" polkadot_sdk_base + "polkadot-omni-node" + darwin_suffix)

[private]
download-chain-spec-builder: (_download "chain-spec-builder" polkadot_sdk_base + "chain-spec-builder" + darwin_suffix)

[private]
download-zombienet: (_download "zombienet" "https://github.com/paritytech/zombienet/releases/latest/download/" + zombienet_asset)

[private]
check: download-binaries
    @echo "Checking prerequisites..."
    @command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found"; exit 1; }
    @echo "All prerequisites found!"

# Start the blockchain (relay chain + parachain)
start-chain: check build
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Blockchain (Relay Chain + Parachain) ==="
    echo ""
    echo "Web UIs (once ready):"
    echo "  Relay chain: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900"
    echo "  Parachain:   https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222"
    echo ""
    .bin/zombienet spawn zombienet.toml

# Build only the provider node
build-provider:
    cargo build --release -p storage-provider-node

# Start the storage provider node
start-provider SEED="//Alice" CHAIN_WS="ws://127.0.0.1:2222": build-provider
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Storage Provider Node ==="
    echo ""
    echo "Provider health: http://127.0.0.1:{{ PORT }}/health"
    echo ""
    SEED="{{SEED}}" \
    CHAIN_RPC="{{CHAIN_WS}}" \
    BIND_ADDR="0.0.0.0:{{ PORT }}" \
    ./target/release/storage-provider-node

# Health check for provider node
health:
    curl -s http://localhost:3333/health | jq .

# Storage stats for provider node
stats:
    curl -s http://localhost:3333/stats | jq .

# Demo: full integration test (PAPI-based)
# Runs setup, upload, 2 challenges + responses, and asserts 2 ChallengeDefended events.
# Requires: npm install in examples/papi/ and descriptors generated (just papi-setup).
demo CHAIN_WS="ws://127.0.0.1:2222" PROVIDER_URL="http://127.0.0.1:3333": papi-setup
    node examples/papi/full-flow.js "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

# Install PAPI dependencies and generate chain descriptors (requires running chain)
papi-setup:
    #!/usr/bin/env bash
    set -euo pipefail
    cd examples/papi
    npm install
    npm run papi:generate

# Generate chain spec
generate-chain-spec: build
    ./scripts/build-chain-spec.sh > chain-spec.json
    @echo "Chain spec generated: chain-spec.json"

# Setup development environment (download binaries + build)
setup: download-binaries build
    @echo ""
    @echo "Setup complete! Run 'just start-chain' and 'just start-provider' to start the local network."
