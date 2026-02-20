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

# Start the storage provider node
start-provider SEED="//Alice" CHAIN_WS="ws://127.0.0.1:2222": build
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

# ============================================================
# File System (Layer 1) Commands
# ============================================================

# Run the file system basic usage example
fs-example:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "🚀 Running File System Client Example"
    echo "Prerequisites: blockchain and provider must be running"
    echo "  - Parachain: ws://127.0.0.1:9944"
    echo "  - Provider: http://localhost:3000"
    echo ""
    cd storage-interfaces/file-system/client
    RUST_LOG=info cargo run --example basic_usage

# Test file system client (unit tests)
fs-test:
    cargo test -p file-system-client

# Test file system client with logs
fs-test-verbose:
    RUST_LOG=debug cargo test -p file-system-client -- --nocapture

# Test all file system components (primitives + pallet + client)
fs-test-all:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Testing file system primitives..."
    cargo test -p file-system-primitives
    echo ""
    echo "Testing drive registry pallet..."
    cargo test -p pallet-drive-registry
    echo ""
    echo "Testing file system client..."
    cargo test -p file-system-client
    echo ""
    echo "✅ All file system tests passed!"

# Start infrastructure and run file system example (full integration test)
fs-integration-test:
    #!/usr/bin/env bash
    set -euo pipefail

    echo ""
    echo "=== File System Integration Test ==="
    echo ""
    echo "This will:"
    echo "  1. Start relay chain + parachain"
    echo "  2. Start provider node"
    echo "  3. Verify on-chain setup"
    echo "  4. Run file system example"
    echo ""

    # Check if zombienet is already running
    if lsof -i :9944 > /dev/null 2>&1; then
        echo "⚠️  Parachain already running on port 9944"
        echo "Skipping blockchain startup..."
    else
        echo "Starting blockchain network..."
        .bin/zombienet spawn zombienet.toml > /tmp/zombienet.log 2>&1 &
        ZOMBIENET_PID=$!
        trap "kill $ZOMBIENET_PID 2>/dev/null || true" EXIT

        echo "Waiting for parachain to be ready..."
        until curl -s -o /dev/null http://127.0.0.1:9944; do
            sleep 2
        done
        echo "✅ Blockchain ready!"
    fi

    # Check if provider is already running
    if lsof -i :3000 > /dev/null 2>&1; then
        echo "⚠️  Provider already running on port 3000"
        echo "Skipping provider startup..."
    else
        echo ""
        echo "Starting provider node..."
        PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY \
        CHAIN_RPC=ws://127.0.0.1:9944 \
        cargo run --release -p storage-provider-node > /tmp/provider.log 2>&1 &
        PROVIDER_PID=$!
        trap "kill $PROVIDER_PID 2>/dev/null || true; kill $ZOMBIENET_PID 2>/dev/null || true" EXIT

        # Wait for provider to be ready
        echo "Waiting for provider to be ready..."
        for i in {1..30}; do
            if curl -s http://localhost:3000/health > /dev/null 2>&1; then
                echo "✅ Provider ready!"
                break
            fi
            if [ $i -eq 30 ]; then
                echo "❌ Provider failed to start"
                exit 1
            fi
            sleep 1
        done
    fi

    echo ""
    echo "Verifying on-chain setup..."
    bash scripts/verify-setup.sh || {
        echo ""
        echo "⚠️  Setup verification failed"
        echo "You may need to run the setup manually. See:"
        echo "  docs/getting-started/QUICKSTART.md"
        echo ""
        echo "Continuing anyway to test drive creation..."
    }

    echo ""
    echo "=== Running File System Example ==="
    echo ""
    just fs-example

    echo ""
    echo "✅ Integration test complete!"

# Quick file system demo (assumes infrastructure is running)
fs-demo:
    #!/usr/bin/env bash
    set -euo pipefail

    # Check prerequisites
    if ! curl -s http://localhost:3000/health > /dev/null 2>&1; then
        echo "❌ Provider not running on http://localhost:3000"
        echo "Run: just start-services"
        exit 1
    fi

    if ! curl -s -o /dev/null http://127.0.0.1:9944; then
        echo "❌ Parachain not running on ws://127.0.0.1:9944"
        echo "Run: just start-chain"
        exit 1
    fi

    echo "✅ Infrastructure is running"
    echo ""
    just fs-example

# Build file system components only
fs-build:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Building file system components..."
    cargo build --release \
        -p file-system-primitives \
        -p pallet-drive-registry \
        -p file-system-client
    echo "✅ File system components built!"

# Clean file system build artifacts
fs-clean:
    cargo clean -p file-system-primitives
    cargo clean -p pallet-drive-registry
    cargo clean -p file-system-client

# Show file system documentation
fs-docs:
    @echo "📚 File System Interface Documentation"
    @echo ""
    @echo "Getting Started:"
    @echo "  docs/filesystems/README.md"
    @echo ""
    @echo "User Guide:"
    @echo "  docs/filesystems/USER_GUIDE.md"
    @echo ""
    @echo "Example Walkthrough:"
    @echo "  docs/filesystems/EXAMPLE_WALKTHROUGH.md"
    @echo ""
    @echo "API Reference:"
    @echo "  docs/filesystems/API_REFERENCE.md"
    @echo ""
    @echo "Client SDK:"
    @echo "  storage-interfaces/file-system/client/README.md"
