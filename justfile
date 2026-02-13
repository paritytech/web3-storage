# Scalable Web3 Storage - Development Commands
#
# Install just:
#   cargo install just
# Or on macOS:
#   brew install just

# Polkadot SDK version (matches Cargo.toml tag)
polkadot_version := "polkadot-stable2512"

# Default recipe
default:
    @just --list

# Build the project
build:
    cargo build --release

# Detect OS and architecture
os := `uname -s | tr '[:upper:]' '[:lower:]'`
arch := `uname -m`

# Download all required binaries
download-binaries: download-polkadot download-polkadot-omni-node download-chain-spec-builder download-zombienet
    @echo "All binaries downloaded to .bin/"

# Download polkadot binaries (polkadot + workers)
download-polkadot:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .bin

    # Download polkadot
    if [[ -x .bin/polkadot ]]; then
        echo "polkadot already exists in .bin/"
    else
        echo "Downloading polkadot for {{os}}/{{arch}}..."
        if [[ "{{os}}" == "darwin" ]]; then
            curl -L -o .bin/polkadot "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-aarch64-apple-darwin"
        else
            curl -L -o .bin/polkadot "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot"
        fi
        chmod +x .bin/polkadot
        echo "polkadot downloaded to .bin/polkadot"
    fi

    # Download polkadot-execute-worker
    if [[ -x .bin/polkadot-execute-worker ]]; then
        echo "polkadot-execute-worker already exists in .bin/"
    else
        echo "Downloading polkadot-execute-worker for {{os}}/{{arch}}..."
        if [[ "{{os}}" == "darwin" ]]; then
            curl -L -o .bin/polkadot-execute-worker "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-execute-worker-aarch64-apple-darwin"
        else
            curl -L -o .bin/polkadot-execute-worker "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-execute-worker"
        fi
        chmod +x .bin/polkadot-execute-worker
        echo "polkadot-execute-worker downloaded to .bin/polkadot-execute-worker"
    fi

    # Download polkadot-prepare-worker
    if [[ -x .bin/polkadot-prepare-worker ]]; then
        echo "polkadot-prepare-worker already exists in .bin/"
    else
        echo "Downloading polkadot-prepare-worker for {{os}}/{{arch}}..."
        if [[ "{{os}}" == "darwin" ]]; then
            curl -L -o .bin/polkadot-prepare-worker "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-prepare-worker-aarch64-apple-darwin"
        else
            curl -L -o .bin/polkadot-prepare-worker "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-prepare-worker"
        fi
        chmod +x .bin/polkadot-prepare-worker
        echo "polkadot-prepare-worker downloaded to .bin/polkadot-prepare-worker"
    fi

# Download polkadot-omni-node binary
download-polkadot-omni-node:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -x .bin/polkadot-omni-node ]]; then
        echo "polkadot-omni-node already exists in .bin/"
        exit 0
    fi
    mkdir -p .bin
    echo "Downloading polkadot-omni-node for {{os}}/{{arch}}..."
    if [[ "{{os}}" == "darwin" ]]; then
        curl -L -o .bin/polkadot-omni-node "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-omni-node-aarch64-apple-darwin"
    else
        curl -L -o .bin/polkadot-omni-node "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-omni-node"
    fi
    chmod +x .bin/polkadot-omni-node
    echo "polkadot-omni-node downloaded to .bin/polkadot-omni-node"

# Download chain-spec-builder binary
download-chain-spec-builder:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -x .bin/chain-spec-builder ]]; then
        echo "chain-spec-builder already exists in .bin/"
        exit 0
    fi
    mkdir -p .bin
    echo "Downloading chain-spec-builder for {{os}}/{{arch}}..."
    if [[ "{{os}}" == "darwin" ]]; then
        curl -L -o .bin/chain-spec-builder "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/chain-spec-builder-aarch64-apple-darwin"
    else
        curl -L -o .bin/chain-spec-builder "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/chain-spec-builder"
    fi
    chmod +x .bin/chain-spec-builder
    echo "chain-spec-builder downloaded to .bin/chain-spec-builder"

# Download zombienet binary
download-zombienet:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -x .bin/zombienet ]]; then
        echo "zombienet already exists in .bin/"
        exit 0
    fi
    mkdir -p .bin
    echo "Downloading zombienet for {{os}}/{{arch}}..."
    if [[ "{{os}}" == "darwin" ]]; then
        if [[ "{{arch}}" == "arm64" ]]; then
            curl -L -o .bin/zombienet "https://github.com/paritytech/zombienet/releases/latest/download/zombienet-macos-arm64"
        else
            curl -L -o .bin/zombienet "https://github.com/paritytech/zombienet/releases/latest/download/zombienet-macos-x64"
        fi
    else
        curl -L -o .bin/zombienet "https://github.com/paritytech/zombienet/releases/latest/download/zombienet-linux-x64"
    fi
    chmod +x .bin/zombienet
    echo "zombienet downloaded to .bin/zombienet"

# Check prerequisites for local environment (downloads binaries if missing)
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
    echo "  Parachain:   https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944"
    echo ""
    .bin/zombienet spawn zombienet.toml

# Start the storage provider node
start-provider SEED="//Alice" CHAIN_WS="ws://127.0.0.1:9944": build
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Storage Provider Node ==="
    echo ""
    echo "Provider health: http://127.0.0.1:3000/health"
    echo ""
    SEED="{{SEED}}" \
    CHAIN_RPC="{{CHAIN_WS}}" \
    cargo run --release -p storage-provider-node

# Health check for provider node
health:
    curl -s http://localhost:3000/health | jq .

# Storage stats for provider node
stats:
    curl -s http://localhost:3000/stats | jq .

# Demo: setup bucket and storage agreement (run once before demo-upload)
demo-setup CHAIN_WS="ws://127.0.0.1:9944" PROVIDER_URL="http://127.0.0.1:3000":
    cargo run --release -p storage-client --bin demo_setup -- "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

# Demo: upload test data to provider (includes timestamp by default)
demo-upload PROVIDER_URL="http://127.0.0.1:3000" BUCKET_ID="1" CHAIN_WS="ws://127.0.0.1:9944":
    #!/usr/bin/env bash
    cargo run --release -p storage-client --bin demo_upload -- "{{PROVIDER_URL}}" "{{BUCKET_ID}}" "{{CHAIN_WS}}" "Hello, Web3 Storage! [$(date -Iseconds)]"

# Demo: challenge a storage provider (verify they have the data)
# For off-chain challenge, provide MMR_ROOT, START_SEQ, and SIGNATURE
demo-challenge CHAIN_WS="ws://127.0.0.1:9944" BUCKET_ID="1" PROVIDER="5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY" LEAF="0" CHUNK="0" MMR_ROOT="" START_SEQ="0" SIGNATURE="":
    #!/usr/bin/env bash
    if [ -n "{{MMR_ROOT}}" ] && [ -n "{{SIGNATURE}}" ]; then
        cargo run --release -p storage-client --bin demo_challenge -- "{{CHAIN_WS}}" "{{BUCKET_ID}}" "{{PROVIDER}}" "{{LEAF}}" "{{CHUNK}}" "{{MMR_ROOT}}" "{{START_SEQ}}" "{{SIGNATURE}}"
    else
        cargo run --release -p storage-client --bin demo_challenge -- "{{CHAIN_WS}}" "{{BUCKET_ID}}" "{{PROVIDER}}" "{{LEAF}}" "{{CHUNK}}"
    fi

# Start the challenge watcher (auto-responds to challenges)
start-watcher SEED="//Alice" CHAIN_WS="ws://127.0.0.1:9944" PROVIDER_URL="http://127.0.0.1:3000":
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Challenge Watcher ==="
    echo ""
    echo "Provider:  {{PROVIDER_URL}}"
    echo "Chain:     {{CHAIN_WS}}"
    echo ""
    SEED="{{SEED}}" \
    CHAIN_WS="{{CHAIN_WS}}" \
    PROVIDER_URL="{{PROVIDER_URL}}" \
    cargo run --release -q -p storage-client --bin challenge_watcher

# Demo: full workflow - setup, upload, checkpoint, challenge with watcher auto-response
demo PROVIDER_URL="http://127.0.0.1:3000" BUCKET_ID="1" CHAIN_WS="ws://127.0.0.1:9944":
    #!/usr/bin/env bash
    set -euo pipefail

    echo "=== Step 1: Setup bucket and agreement ==="
    cargo run --release -q -p storage-client --bin demo_setup -- "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

    echo ""
    echo "=== Step 2: Upload data ==="
    OUTPUT=$(cargo run --release -q -p storage-client --bin demo_upload -- "{{PROVIDER_URL}}" "{{BUCKET_ID}}" "{{CHAIN_WS}}" "Hello, Web3 Storage! [$(date -Iseconds)]" 2>&1)
    echo "$OUTPUT"

    # Extract JSON from output (from line starting with '{' to the end)
    JSON=$(echo "$OUTPUT" | awk '/^{/,0')

    if [ -z "$JSON" ]; then
        echo "Error: Could not parse JSON from upload output"
        exit 1
    fi

    # Extract challenge parameters from upload JSON
    LEAF_INDEX=$(echo "$JSON" | jq -r '.leaf_indices[0]')
    PROVIDER="5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    MMR_ROOT=$(echo "$JSON" | jq -r '.mmr_root')
    START_SEQ=$(echo "$JSON" | jq -r '.start_seq')
    SIGNATURE=$(echo "$JSON" | jq -r '.provider_signature')

    echo ""
    echo "=== Step 3: Challenge provider (off-chain) ==="
    echo "Challenging with:"
    echo "  bucket_id={{BUCKET_ID}}"
    echo "  provider=$PROVIDER"
    echo "  leaf=$LEAF_INDEX"
    echo "  mmr_root=$MMR_ROOT"
    echo "  start_seq=$START_SEQ"
    echo "  signature=${SIGNATURE:0:20}..."
    echo ""

    cargo run --release -q -p storage-client --bin demo_challenge -- "{{CHAIN_WS}}" "{{BUCKET_ID}}" "$PROVIDER" "$LEAF_INDEX" "0" "$MMR_ROOT" "$START_SEQ" "$SIGNATURE"

    echo ""
    echo "=== Step 4: Start challenge watcher (background) ==="
    SEED="//Alice" CHAIN_WS="{{CHAIN_WS}}" PROVIDER_URL="{{PROVIDER_URL}}" \
        cargo run --release -q -p storage-client --bin challenge_watcher &
    WATCHER_PID=$!
    echo "Watcher PID: $WATCHER_PID"
    sleep 3

    echo ""
    echo "=== Step 5: Submit on-chain checkpoint ==="
    cargo run --release -q -p storage-client --bin demo_checkpoint -- "{{CHAIN_WS}}" "{{BUCKET_ID}}" "{{PROVIDER_URL}}" "$PROVIDER"

    echo ""
    echo "=== Step 6: Challenge provider (on-chain checkpoint) ==="
    echo "The watcher should auto-respond to this challenge..."
    cargo run --release -q -p storage-client --bin demo_challenge -- "{{CHAIN_WS}}" "{{BUCKET_ID}}" "$PROVIDER" "$LEAF_INDEX" "0"

    echo ""
    echo "=== Waiting for watcher to respond (30s) ==="
    sleep 30

    # Stop watcher
    kill $WATCHER_PID 2>/dev/null || true
    echo ""
    echo "=== Demo complete! ==="

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
