# Scalable Web3 Storage - Development Commands
#
# Install just:
#   cargo install just
# Or on macOS:
#   brew install just

# Polkadot SDK version (matches Cargo.toml tag)
polkadot_version := "polkadot-stable2603"
# Zombienet version
zombienet_version := "v0.4.11"

# Detect OS and architecture
os := `uname -s | tr '[:upper:]' '[:lower:]'`
arch := `uname -m`

# URL components
polkadot_sdk_base := "https://github.com/paritytech/polkadot-sdk/releases/download/" + polkadot_version + "/"
darwin_suffix := if os == "darwin" { "-aarch64-apple-darwin" } else { "" }
zombienet_asset := if os == "darwin" { "zombie-cli-aarch64-apple-darwin" } else { "zombie-cli-x86_64-unknown-linux-musl" }

# Network ports (override with: just PROVIDER_PORT=3001 start-provider)
RELAY_PORT := "9900"
CHAIN_PORT := "2222"
PROVIDER_PORT := "3333"

# Network URLs (constructed from ports)
RELAY_WS := "ws://127.0.0.1:" + RELAY_PORT
CHAIN_WS := "ws://127.0.0.1:" + CHAIN_PORT
PROVIDER_URL := "http://127.0.0.1:" + PROVIDER_PORT
PROVIDER_MULTI_ADDR := "/ip4/127.0.0.1/tcp/" + PROVIDER_PORT

# Default recipe
default:
    @just --list

# Build the project
build:
    cargo build --release

# Build only the runtime
build-runtime:
    cargo build --release -p storage-parachain-runtime

# Build only the paseo runtime
build-paseo-runtime:
    cargo build --release -p storage-paseo-runtime

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
download-binaries: download-polkadot-sdk-binaries download-zombienet
    @echo "All binaries downloaded to .bin/"

# Download Polkadot SDK binaries (polkadot, omni-node, chain-spec-builder)
download-polkadot-sdk-binaries: _download-polkadot _download-polkadot-omni-node _download-chain-spec-builder

# Download zombienet
download-zombienet: (_download "zombienet" "https://github.com/paritytech/zombienet-sdk/releases/download/" + zombienet_version + "/" + zombienet_asset)

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

# Setup development environment (download binaries + build)
setup: download-binaries build
    @echo ""
    @echo "Setup complete! Run 'just start-chain' and 'just start-provider' to start the local network."

# Start the blockchain (relay chain + parachain)
start-chain: check build-runtime
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Blockchain (Relay Chain + Parachain) ==="
    echo ""
    PROJECT_ROOT=$(pwd) .bin/zombienet spawn -p native zombienet.toml

# Start the blockchain (relay chain + paseo storage parachain)
start-paseo-chain: check build-paseo-runtime
    #!/usr/bin/env bash
    echo ""
    echo "=== Starting Blockchain (Relay Chain + Paseo Storage Parachain) ==="
    echo ""
    PROJECT_ROOT=$(pwd) .bin/zombienet spawn -p native zombienet/storage-paseo-local.toml

# Start the storage provider node
# Examples:
#   just start-provider                                       # inmemory, //Alice key, port 3333
#   just start-provider MODE=disk PORT=3334                    # disk storage on port 3334
#   just start-provider KEYFILE=/path/to/seed MODE=disk        # custom key from file
start-provider MODE="inmemory" PORT=PROVIDER_PORT STORAGE_PATH="./provider-data" KEYFILE="": build-provider
    #!/usr/bin/env bash
    set -euo pipefail
    echo ""
    echo "=== Starting Storage Provider Node ({{MODE}}) ==="
    echo ""
    echo "Provider health: http://127.0.0.1:{{PORT}}/health"
    echo ""
    EXTRA_ARGS=""
    if [ "{{MODE}}" = "disk" ]; then
        EXTRA_ARGS="--storage-path {{STORAGE_PATH}}"
    fi
    if [ -n "{{KEYFILE}}" ]; then
        KEY_ARGS="--keyfile {{KEYFILE}}"
    else
        ALICE_KEY=$(mktemp)
        echo "//Alice" > "$ALICE_KEY" && chmod 600 "$ALICE_KEY"
        KEY_ARGS="--keyfile $ALICE_KEY"
        trap "rm -f $ALICE_KEY" EXIT
    fi

    just register-provider "{{KEYFILE}}"
    echo ""

    ./target/release/storage-provider-node \
        $KEY_ARGS \
        --storage-mode "{{MODE}}" \
        --bind-addr "0.0.0.0:{{PORT}}" \
        --chain-rpc "{{ CHAIN_WS }}" \
        --enable-agreement-coordinator \
        --enable-checkpoint-coordinator \
        $EXTRA_ARGS

# Register provider on-chain (idempotent). Requires a running chain.
# Usually called automatically by start-provider.
register-provider KEYFILE="":
    #!/usr/bin/env bash
    set -euo pipefail
    ARGS=("{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_MULTI_ADDR }}")
    if [ -n "{{KEYFILE}}" ]; then
        ARGS+=("{{KEYFILE}}")
    fi
    cargo run -p storage-client --example register_provider -- "${ARGS[@]}"

# Health check for provider node
health:
    curl -s {{ PROVIDER_URL }}/health | jq .

# Storage stats for provider node
stats:
    curl -s {{ PROVIDER_URL }}/stats | jq .

# Generate chain spec
generate-chain-spec: build-runtime
    ./scripts/build-chain-spec.sh > chain-spec.json
    @echo "Chain spec generated: chain-spec.json"

# Demo: full integration test (PAPI-based)
# Runs setup, upload, 2 challenges + responses, and asserts 2 ChallengeDefended events.
# Requires: npm install in examples/papi/ and descriptors generated (just papi-setup).
# Examples:
#   just demo                                                       # default: Alice provider, Bob client
#   just demo "http://127.0.0.1:3334" "//Charlie" "//Dave"          # target a different provider
demo PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node examples/papi/full-flow.js "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Install PAPI dependencies and generate chain descriptors (requires running chain)
papi-setup:
    #!/usr/bin/env bash
    set -euo pipefail
    cd examples/papi
    npm install
    npm run papi:generate

# ============================================================
# File System (Layer 1)
# ============================================================

# Test all file system components (primitives + pallet + client)
fs-test-all:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test -p file-system-primitives
    cargo test -p pallet-drive-registry
    cargo test -p file-system-client

# File system integration test (used by CI; assumes chain + provider already running)
fs-demo-ci:
    cargo run --release -p file-system-client --example ci_integration_test -- "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}"

# ============================================================
# S3-Compatible Interface (Layer 1)
# ============================================================

# Test all S3 components (primitives + pallet + client)
s3-test-all:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test -p s3-primitives
    cargo test -p pallet-s3-registry
    cargo test -p s3-client

# S3 integration test (used by CI; assumes chain + provider already running)
s3-demo-ci:
    cargo run --release -p s3-client --example ci_integration_test -- "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}"
