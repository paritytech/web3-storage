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

# Demo: full workflow - setup, upload, and challenge
demo PROVIDER_URL="http://127.0.0.1:3000" BUCKET_ID="1" CHAIN_WS="ws://127.0.0.1:9944":
    #!/usr/bin/env bash
    set -euo pipefail

    echo "=== Step 1: Setup bucket and agreement ==="
    cargo run --release -p storage-client --bin demo_setup -- "{{CHAIN_WS}}" "{{PROVIDER_URL}}"

    echo ""
    echo "=== Step 2: Upload data ==="
    OUTPUT=$(cargo run --release -p storage-client --bin demo_upload -- "{{PROVIDER_URL}}" "{{BUCKET_ID}}" "{{CHAIN_WS}}" "Hello, Web3 Storage! [$(date -Iseconds)]" 2>&1)
    echo "$OUTPUT"

    # Extract JSON from output (from line starting with '{' to the end)
    JSON=$(echo "$OUTPUT" | awk '/^{/,0')

    if [ -z "$JSON" ]; then
        echo "Error: Could not parse JSON from upload output"
        exit 1
    fi

    # Extract challenge parameters
    LEAF_INDEX=$(echo "$JSON" | jq -r '.challenge.leaf_index')
    PROVIDER=$(echo "$JSON" | jq -r '.challenge.provider_account')
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

    cargo run --release -p storage-client --bin demo_challenge -- "{{CHAIN_WS}}" "{{BUCKET_ID}}" "$PROVIDER" "$LEAF_INDEX" "0" "$MMR_ROOT" "$START_SEQ" "$SIGNATURE"

# Generate chain spec
generate-chain-spec: build
    ./scripts/build-chain-spec.sh > chain-spec.json
    @echo "Chain spec generated: chain-spec.json"

# Setup development environment (download binaries + build)
setup: download-binaries build
    @echo ""
    @echo "Setup complete! Run 'just start-services' to start the local network."
