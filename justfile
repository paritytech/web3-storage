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

    # Determine platform suffix (only arm64 macOS binaries are available)
    if [[ "{{os}}" == "darwin" ]]; then
        if [[ "{{arch}}" != "arm64" ]]; then
            echo "Error: Only arm64 macOS binaries are available. x86_64 macOS is not supported."
            exit 1
        fi
        SUFFIX="-aarch64-apple-darwin"
    else
        SUFFIX=""
    fi

    # Download polkadot
    if [[ -x .bin/polkadot ]]; then
        echo "polkadot already exists in .bin/"
    else
        echo "Downloading polkadot for {{os}}/{{arch}}..."
        curl -L -o .bin/polkadot "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot${SUFFIX}"
        chmod +x .bin/polkadot
        echo "polkadot downloaded to .bin/polkadot"
    fi

    # Download polkadot-execute-worker
    if [[ -x .bin/polkadot-execute-worker ]]; then
        echo "polkadot-execute-worker already exists in .bin/"
    else
        echo "Downloading polkadot-execute-worker for {{os}}/{{arch}}..."
        curl -L -o .bin/polkadot-execute-worker "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-execute-worker${SUFFIX}"
        chmod +x .bin/polkadot-execute-worker
        echo "polkadot-execute-worker downloaded to .bin/polkadot-execute-worker"
    fi

    # Download polkadot-prepare-worker
    if [[ -x .bin/polkadot-prepare-worker ]]; then
        echo "polkadot-prepare-worker already exists in .bin/"
    else
        echo "Downloading polkadot-prepare-worker for {{os}}/{{arch}}..."
        curl -L -o .bin/polkadot-prepare-worker "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-prepare-worker${SUFFIX}"
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

    # Determine platform suffix (only arm64 macOS binaries are available)
    if [[ "{{os}}" == "darwin" ]]; then
        if [[ "{{arch}}" != "arm64" ]]; then
            echo "Error: Only arm64 macOS binaries are available. x86_64 macOS is not supported."
            exit 1
        fi
        SUFFIX="-aarch64-apple-darwin"
    else
        SUFFIX=""
    fi

    echo "Downloading polkadot-omni-node for {{os}}/{{arch}}..."
    curl -L -o .bin/polkadot-omni-node "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/polkadot-omni-node${SUFFIX}"
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

    # Determine platform suffix (only arm64 macOS binaries are available)
    if [[ "{{os}}" == "darwin" ]]; then
        if [[ "{{arch}}" != "arm64" ]]; then
            echo "Error: Only arm64 macOS binaries are available. x86_64 macOS is not supported."
            exit 1
        fi
        SUFFIX="-aarch64-apple-darwin"
    else
        SUFFIX=""
    fi

    echo "Downloading chain-spec-builder for {{os}}/{{arch}}..."
    curl -L -o .bin/chain-spec-builder "https://github.com/paritytech/polkadot-sdk/releases/download/{{polkadot_version}}/chain-spec-builder${SUFFIX}"
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
    .bin/zombienet spawn zombienet.toml

# Start all services (zombienet + provider node)
start-services: check build
    #!/usr/bin/env bash
    set -euo pipefail

    echo ""
    echo "=== Starting Local Development Environment ==="
    echo ""
    echo "Web UIs (once ready):"
    echo "  Relay chain:    https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900"
    echo "  Parachain:      https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944"
    echo "  Provider health: http://127.0.0.1:3000/health"
    echo ""

    # Start zombienet in background
    .bin/zombienet spawn zombienet.toml &
    ZOMBIENET_PID=$!

    # Cleanup on exit
    trap "kill $ZOMBIENET_PID 2>/dev/null" EXIT

    # Wait for parachain RPC to be available
    echo "Waiting for parachain to be ready..."
    until curl -s -o /dev/null http://127.0.0.1:9944; do
        sleep 2
    done
    echo "Parachain is ready!"
    echo ""

    # Start provider node in foreground
    echo "Starting provider node..."
    PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY \
    CHAIN_RPC=ws://127.0.0.1:9944 \
    cargo run --release -p storage-provider-node

# Health check for provider node
health:
    curl -s http://localhost:3000/health | jq .

# Generate chain spec
generate-chain-spec: build
    ./scripts/build-chain-spec.sh > chain-spec.json
    @echo "Chain spec generated: chain-spec.json"

# Setup development environment (download binaries + build)
setup: download-binaries build
    @echo ""
    @echo "Setup complete! Run 'just start-services' to start the local network."
