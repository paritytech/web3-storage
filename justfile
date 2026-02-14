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

# Default recipe
default:
    @just --list

# Build the project
build:
    cargo build --release

[private]
build-examples:
    cargo build --release -p storage-examples

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
    ./target/release/storage-provider-node

# Health check for provider node
health:
    curl -s http://localhost:3000/health | jq .

# Storage stats for provider node
stats:
    curl -s http://localhost:3000/stats | jq .

# Demo: setup bucket and storage agreement (run once before demo-upload)
demo-setup CHAIN_WS="ws://127.0.0.1:9944" PROVIDER_URL="http://127.0.0.1:3000": build-examples
    ./target/release/demo_setup "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

# Demo: upload test data to provider (includes timestamp by default)
demo-upload PROVIDER_URL="http://127.0.0.1:3000" BUCKET_ID="1" CHAIN_WS="ws://127.0.0.1:9944": build-examples
    #!/usr/bin/env bash
    ./target/release/demo_upload "{{PROVIDER_URL}}" "{{BUCKET_ID}}" "{{CHAIN_WS}}" "Hello, Web3 Storage! [$(date -Iseconds)]"

# Demo: challenge a storage provider (verify they have the data)
# For off-chain challenge, provide MMR_ROOT, START_SEQ, and SIGNATURE
demo-challenge CHAIN_WS="ws://127.0.0.1:9944" BUCKET_ID="1" PROVIDER="5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY" LEAF="0" CHUNK="0" MMR_ROOT="" START_SEQ="0" SIGNATURE="": build-examples
    #!/usr/bin/env bash
    if [ -n "{{MMR_ROOT}}" ] && [ -n "{{SIGNATURE}}" ]; then
        ./target/release/demo_challenge "{{CHAIN_WS}}" "{{BUCKET_ID}}" "{{PROVIDER}}" "{{LEAF}}" "{{CHUNK}}" "{{MMR_ROOT}}" "{{START_SEQ}}" "{{SIGNATURE}}"
    else
        ./target/release/demo_challenge "{{CHAIN_WS}}" "{{BUCKET_ID}}" "{{PROVIDER}}" "{{LEAF}}" "{{CHUNK}}"
    fi

# Start the challenge watcher (auto-responds to challenges)
start-watcher SEED="//Alice" CHAIN_WS="ws://127.0.0.1:9944" PROVIDER_URL="http://127.0.0.1:3000": build-examples
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
    ./target/release/challenge_watcher

# Demo: full integration test (PAPI-based)
# Runs setup, upload, 2 challenges + responses, and asserts 2 ChallengeDefended events.
# Requires: npm install in examples/papi/ and descriptors generated (just papi-setup).
demo CHAIN_WS="ws://127.0.0.1:9944" PROVIDER_URL="http://127.0.0.1:3000":
    node examples/papi/demo.js "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

# Demo: full workflow using Rust binaries (legacy, no assertions)
demo-legacy PROVIDER_URL="http://127.0.0.1:3000" BUCKET_ID="1" CHAIN_WS="ws://127.0.0.1:9944": build-examples
    #!/usr/bin/env bash
    set -euo pipefail

    echo "=== Step 1: Setup bucket and agreement ==="
    ./target/release/demo_setup "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

    echo ""
    echo "=== Step 2: Upload data ==="
    OUTPUT=$(./target/release/demo_upload "{{PROVIDER_URL}}" "{{BUCKET_ID}}" "{{CHAIN_WS}}" "Hello, Web3 Storage! [$(date -Iseconds)]" 2>&1)
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

    ./target/release/demo_challenge "{{CHAIN_WS}}" "{{BUCKET_ID}}" "$PROVIDER" "$LEAF_INDEX" "0" "$MMR_ROOT" "$START_SEQ" "$SIGNATURE"

    echo ""
    echo "=== Step 4: Start challenge watcher (background) ==="
    WATCHER_LOG=$(mktemp)
    SEED="//Alice" CHAIN_WS="{{CHAIN_WS}}" PROVIDER_URL="{{PROVIDER_URL}}" \
        ./target/release/challenge_watcher 2>"$WATCHER_LOG" &
    WATCHER_PID=$!
    echo "Watcher PID: $WATCHER_PID (log: $WATCHER_LOG)"
    sleep 3

    echo ""
    echo "=== Step 5: Submit on-chain checkpoint ==="
    ./target/release/demo_checkpoint "{{CHAIN_WS}}" "{{BUCKET_ID}}" "{{PROVIDER_URL}}" "$PROVIDER"

    echo ""
    echo "=== Step 6: Challenge provider (on-chain checkpoint) ==="
    echo "The watcher should auto-respond to this challenge..."
    ./target/release/demo_challenge "{{CHAIN_WS}}" "{{BUCKET_ID}}" "$PROVIDER" "$LEAF_INDEX" "0"

    echo ""
    echo "=== Waiting for watcher to defend both challenges ==="
    for i in $(seq 1 60); do
        DEFENDED_COUNT=$(grep -c "defended successfully" "$WATCHER_LOG" || true)
        if [ "$DEFENDED_COUNT" -ge 2 ]; then
            echo "Both challenges defended (attempt $i)"
            break
        fi
        if [ "$i" -eq 60 ]; then
            echo "Timeout waiting for challenge responses"
        fi
        sleep 2
    done

    # Stop watcher
    kill $WATCHER_PID 2>/dev/null || true

    echo ""
    echo "=== Watcher log ==="
    cat "$WATCHER_LOG"

    echo ""
    echo "=== Verifying challenge responses ==="
    DEFENDED_COUNT=$(grep -c "defended successfully" "$WATCHER_LOG" || true)
    echo "ChallengeDefended events: $DEFENDED_COUNT (expected: 2)"
    rm -f "$WATCHER_LOG"
    if [ "$DEFENDED_COUNT" -ne 2 ]; then
        echo "FAILED: Expected 2 ChallengeDefended events, got $DEFENDED_COUNT"
        exit 1
    fi
    echo "PASSED: Both challenges were defended!"
    echo ""
    echo "=== Demo complete! ==="

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
