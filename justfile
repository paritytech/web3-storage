# Scalable Web3 Storage - Development Commands
#
# Install just:
#   cargo install just
# Or on macOS:
#   brew install just

# Polkadot SDK version (matches Cargo.toml tag)
polkadot_version := "polkadot-stable2512-2"

# Detect OS and architecture
os := `uname -s | tr '[:upper:]' '[:lower:]'`
arch := `uname -m`

# URL components
polkadot_sdk_base := "https://github.com/paritytech/polkadot-sdk/releases/download/" + polkadot_version + "/"
darwin_suffix := if os == "darwin" { "-aarch64-apple-darwin" } else { "" }

# Network ports (override with: just PROVIDER_PORT=3001 start-provider)
RELAY_PORT := "9900"
CHAIN_PORT := "2222"
PROVIDER_PORT := "3333"

# Network URLs (constructed from ports)
RELAY_WS := "ws://127.0.0.1:" + RELAY_PORT
CHAIN_WS := "ws://127.0.0.1:" + CHAIN_PORT
PROVIDER_URL := "http://127.0.0.1:" + PROVIDER_PORT

# Default recipe
default:
    @just --list

# Build the project
build:
    cargo build --release

# Build only the runtime
build-runtime:
    cargo build --release -p storage-parachain-runtime

# Build only the provider node
build-provider:
    cargo build --release -p storage-provider-node

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
download-binaries: download-polkadot-sdk-binaries
    @echo "All binaries downloaded to .bin/"

# Download Polkadot SDK binaries (polkadot, omni-node, chain-spec-builder)
download-polkadot-sdk-binaries: _download-polkadot _download-polkadot-omni-node _download-chain-spec-builder

[private]
_download-polkadot: (_download "polkadot" polkadot_sdk_base + "polkadot" + darwin_suffix) (_download "polkadot-execute-worker" polkadot_sdk_base + "polkadot-execute-worker" + darwin_suffix) (_download "polkadot-prepare-worker" polkadot_sdk_base + "polkadot-prepare-worker" + darwin_suffix)

[private]
_download-polkadot-omni-node: (_download "polkadot-omni-node" polkadot_sdk_base + "polkadot-omni-node" + darwin_suffix)

[private]
_download-chain-spec-builder: (_download "chain-spec-builder" polkadot_sdk_base + "chain-spec-builder" + darwin_suffix)

[private]
check: download-binaries
    @echo "Checking prerequisites..."
    @command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found"; exit 1; }
    @echo "All prerequisites found!"

# Start the blockchain (relay chain + parachain)
start-chain: check build-runtime
    cargo run --release -p zombienet-sdk-tests --bin smoke

# Start the blockchain + storage provider
start-all: check build-runtime build-provider
    cargo run --release -p zombienet-sdk-tests --bin smoke -- --with-provider

# Start the storage provider node
start-provider SEED="//Alice": build-provider
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Storage Provider Node ==="
    echo ""
    echo "Provider health: {{ PROVIDER_URL }}/health"
    echo ""
    SEED="{{SEED}}" \
    CHAIN_RPC="{{ CHAIN_WS }}" \
    BIND_ADDR="0.0.0.0:{{ PROVIDER_PORT }}" \
    ./target/release/storage-provider-node

# Health check for provider node
health:
    curl -s {{ PROVIDER_URL }}/health | jq .

# Storage stats for provider node
stats:
    curl -s {{ PROVIDER_URL }}/stats | jq .

# Layer 0 integration test: full storage flow (zombienet-sdk based)
demo: build-runtime build-provider
    cargo test --release -p zombienet-sdk-tests --features zombie-tests layer0 -- --nocapture

# Layer 1 integration test: file system flow (zombienet-sdk based)
fs-demo-ci: build-runtime build-provider
    cargo test --release -p zombienet-sdk-tests --features zombie-tests layer1::filesystem -- --nocapture

# Layer 1 integration test: S3 flow (zombienet-sdk based)
s3-demo-ci: build-runtime build-provider
    cargo test --release -p zombienet-sdk-tests --features zombie-tests layer1::s3 -- --nocapture

# Generate chain spec
generate-chain-spec: build-runtime
    ./scripts/build-chain-spec.sh > chain-spec.json
    @echo "Chain spec generated: chain-spec.json"

# Setup development environment (download binaries + build)
setup: download-binaries build
    @echo ""
    @echo "Setup complete! Run 'just start-chain' to start the local network."

# ============================================================
# File System (Layer 1) Commands
# ============================================================

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

# ============================================================
# S3-Compatible Interface (Layer 1) Commands
# ============================================================

# Run the S3 client basic usage example
s3-example:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "🚀 Running S3 Client Example"
    echo "Prerequisites: blockchain and provider must be running"
    echo "  - Parachain: ws://127.0.0.1:2222"
    echo "  - Provider: http://localhost:3333"
    echo ""
    cd storage-interfaces/s3/client
    RUST_LOG=info cargo run --example basic_usage

# Test S3 primitives
s3-test-primitives:
    cargo test -p s3-primitives

# Test S3 registry pallet
s3-test-pallet:
    cargo test -p pallet-s3-registry

# Test S3 client (unit tests)
s3-test:
    cargo test -p s3-client

# Test S3 client with logs
s3-test-verbose:
    RUST_LOG=debug cargo test -p s3-client -- --nocapture

# Test all S3 components (primitives + pallet + client)
s3-test-all:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Testing S3 primitives..."
    cargo test -p s3-primitives
    echo ""
    echo "Testing S3 registry pallet..."
    cargo test -p pallet-s3-registry
    echo ""
    echo "Testing S3 client..."
    cargo test -p s3-client
    echo ""
    echo "✅ All S3 tests passed!"

# Build S3 components only
s3-build:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Building S3 components..."
    cargo build --release \
        -p s3-primitives \
        -p pallet-s3-registry \
        -p s3-client
    echo "✅ S3 components built!"

# Clean S3 build artifacts
s3-clean:
    cargo clean -p s3-primitives
    cargo clean -p pallet-s3-registry
    cargo clean -p s3-client
