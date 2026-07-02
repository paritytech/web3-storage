# Scalable Web3 Storage - Development Commands
#
# Install just:
#   cargo install just
# Or on macOS:
#   brew install just

# Tool versions come from .github/env — the single source of truth shared with
# CI. Shell-exported vars still override file values.
set dotenv-load := true
set dotenv-filename := ".github/env"

# Polkadot SDK version (matches Cargo.toml tag)
polkadot_version := env("POLKADOT_SDK_VERSION")
# Zombienet version
zombienet_version := env("ZOMBIENET_VERSION")
# try-runtime CLI version (for runtime migration checks)
try_runtime_version := env("TRY_RUNTIME_VERSION")

# Detect OS and architecture
os := `uname -s | tr '[:upper:]' '[:lower:]'`
arch := `uname -m`

# URL components
polkadot_sdk_base := "https://github.com/paritytech/polkadot-sdk/releases/download/" + polkadot_version + "/"
darwin_suffix := if os == "darwin" { "-aarch64-apple-darwin" } else { "" }
zombienet_asset := if os == "darwin" { "zombie-cli-aarch64-apple-darwin" } else { "zombie-cli-x86_64-unknown-linux-musl" }
try_runtime_base := "https://github.com/paritytech/try-runtime-cli/releases/download/" + try_runtime_version + "/"
try_runtime_asset := if os == "darwin" { "try-runtime-aarch64-apple-darwin" } else { "try-runtime-x86_64-unknown-linux-musl" }
try_runtime_sha256 := if os == "darwin" { env("TRY_RUNTIME_AARCH64_APPLE_DARWIN_SHA256", "") } else { env("TRY_RUNTIME_X86_64_UNKNOWN_LINUX_MUSL_SHA256", "") }

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
_download BIN URL SHA256="":
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .bin
    if [[ -x .bin/{{BIN}} ]]; then
        echo "{{BIN}} already exists in .bin/"
        exit 0
    fi
    echo "Downloading {{BIN}}..."
    for attempt in 1 2 3; do
        echo "  attempt $attempt/3 ..."
        if curl -L --retry 2 --retry-delay 5 --connect-timeout 30 --max-time 600 \
            -o .bin/{{BIN}} "{{URL}}"; then
            break
        fi
        if [ "$attempt" -eq 3 ]; then
            echo "Failed to download {{BIN}} after 3 attempts"
            exit 1
        fi
        sleep $((attempt * 5))
    done
    if [[ -n "{{SHA256}}" ]]; then
        actual=$(if command -v sha256sum >/dev/null 2>&1; then sha256sum .bin/{{BIN}}; else shasum -a 256 .bin/{{BIN}}; fi | cut -d' ' -f1)
        if [[ "$actual" != "{{SHA256}}" ]]; then
            echo "Checksum mismatch for {{BIN}}: expected {{SHA256}}, got $actual"
            rm -f .bin/{{BIN}}
            exit 1
        fi
        echo "{{BIN}} checksum verified"
    fi
    chmod +x .bin/{{BIN}}
    echo "{{BIN}} downloaded to .bin/{{BIN}}"

# Download all required binaries
download-binaries: download-polkadot-sdk-binaries download-zombienet download-frame-omni-bencher
    @echo "All binaries downloaded to .bin/"

# Download Polkadot SDK binaries (polkadot, omni-node, chain-spec-builder)
download-polkadot-sdk-binaries: _download-polkadot _download-polkadot-omni-node _download-chain-spec-builder

# Download zombienet
download-zombienet: (_download "zombienet" "https://github.com/paritytech/zombienet-sdk/releases/download/" + zombienet_version + "/" + zombienet_asset)

# Download frame-omni-bencher (for benchmarks / `/cmd bench`)
download-frame-omni-bencher: (_download "frame-omni-bencher" polkadot_sdk_base + "frame-omni-bencher" + darwin_suffix)

# Download try-runtime CLI (for runtime migration checks)
download-try-runtime: (_download "try-runtime" (try_runtime_base + try_runtime_asset) try_runtime_sha256)

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

# Start a standalone dev chain with 2s blocks for E2E testing (no relay chain).
# Blocks finalize instantly — much faster than zombienet for automated tests.
# The runtime build command and chain-spec script are read from
# scripts/runtimes-matrix.json so a local run matches the CI e2e job exactly.
# Defaults to web3-storage-paseo (what CI's e2e job uses).
start-e2e-chain RUNTIME="web3-storage-paseo": check
    #!/usr/bin/env bash
    set -euo pipefail
    BUILD_CMD=$(jq -r --arg r "{{ RUNTIME }}" '.[] | select(.name==$r) | .build_command // empty' scripts/runtimes-matrix.json)
    SPEC_SCRIPT=$(jq -r --arg r "{{ RUNTIME }}" '.[] | select(.name==$r) | .chain_spec_script // empty' scripts/runtimes-matrix.json)
    if [ -z "$BUILD_CMD" ] || [ -z "$SPEC_SCRIPT" ]; then
        echo "Unknown runtime '{{ RUNTIME }}' (no match with build_command + chain_spec_script in scripts/runtimes-matrix.json)"
        exit 1
    fi
    just "$BUILD_CMD"
    echo ""
    echo "=== Starting E2E Dev Chain ({{ RUNTIME }}, 2s blocks, no relay chain) ==="
    echo ""
    SPEC_FILE="/tmp/e2e-chain-spec.json"
    "$SPEC_SCRIPT" > "$SPEC_FILE"
    .bin/polkadot-omni-node \
        --chain "$SPEC_FILE" \
        --alice --tmp \
        --unsafe-force-node-key-generation \
        --dev-block-time 2000 \
        --rpc-port {{ CHAIN_PORT }} \
        --rpc-cors all \
        -lruntime=info

# Start the storage provider node (without registering on-chain)
# Examples:
#   just start-provider                                       # inmemory, //Alice key, port 3333, auth enforced
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

    ./target/release/storage-provider-node \
        $KEY_ARGS \
        --storage-mode "{{MODE}}" \
        --bind-addr "0.0.0.0:{{PORT}}" \
        --chain-rpc "{{ CHAIN_WS }}" \
        --enable-checkpoint-coordinator \
        $EXTRA_ARGS

# Register on-chain then start the provider node (original behavior)
register-then-start-provider MODE="inmemory" PORT=PROVIDER_PORT STORAGE_PATH="./provider-data" KEYFILE="":
    just start-provider MODE="{{MODE}}" PORT="{{PORT}}" STORAGE_PATH="{{STORAGE_PATH}}" KEYFILE="{{KEYFILE}}"
    just register-provider "{{KEYFILE}}"

# Register provider on-chain (idempotent). Requires a running chain.
# Called automatically by register-then-start-provider, or run standalone.
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
# Requires: workspace deps installed (just papi-setup).
# Examples:
#   just demo                                                       # default: Alice provider, Bob client
#   just demo "http://127.0.0.1:3334" "//Charlie" "//Dave"          # target a different provider
demo PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/full-flow.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Compile the example marketplace contract to PolkaVM bytecode + ABI.
# Requires: solc and resolc on PATH (see examples/contracts/README.md).
build-contracts:
    bash examples/contracts/build.sh

# Smart-contract e2e demo: deploy StorageMarketplace, call buyStorage through
# the storage-provider precompile, exercise upload/challenge, end via contract.
# Requires: chain + provider running, contracts built (`just build-contracts`).
sc-demo PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/sc-flow.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Smart-contract precompile-coverage e2e: directly invokes every selector on
# all three precompiles (storage-provider + drive-registry + s3-registry) and
# asserts pallet events/storage updated. No intermediate contract.
# Requires: chain + provider running, contracts built (`just build-contracts`).
sc-coverage PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/sc-coverage.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Smart-contract team-drive demo: deploys SharedTeamDrive, exercises
# createTeam → invite → kick → disband through the drive-registry precompile.
sc-team-drive PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/sc-team-drive.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Photos dApp (Layer 1) — namespaced module. Run `just photos` to list its recipes;
# e.g. `just photos build`, `just photos deploy`, `just photos flow`.
mod photos 'user-interfaces/photos/photos.just'

# Smart-contract token-gated demo: deploys TokenGatedDrive, mints an
# NFT-shaped access token per S3 object through the s3-registry precompile,
# transfers + burns + shuts down.
sc-token-gated PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/sc-token-gated.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Wait until the parachain's transaction pool is empty (bounded ~60s, then
# proceeds with a warning). Run between back-to-back integration tests so the
# next step doesn't pick up an `accountNextIndex` that misses an in-flight tx
# (which would land with the same nonce and get dropped as "Usurped").
drain-pool RPC=CHAIN_WS:
    #!/usr/bin/env bash
    set -euo pipefail
    RPC_INPUT="{{ RPC }}"
    # author_pendingExtrinsics works over plain HTTP; convert ws:// → http://.
    RPC_HTTP="${RPC_INPUT/ws:/http:}"
    RPC_HTTP="${RPC_HTTP/wss:/https:}"
    MAX_ITERS=30
    SLEEP_BETWEEN=2
    FINAL_BUFFER=6 # ~1 block at 6s block time
    echo "drain-pool: polling $RPC_HTTP for author_pendingExtrinsics"
    for i in $(seq 1 "$MAX_ITERS"); do
        RESPONSE=$(curl -s -H "Content-Type: application/json" \
            -d '{"id":1,"jsonrpc":"2.0","method":"author_pendingExtrinsics","params":[]}' \
            "$RPC_HTTP" 2>/dev/null || true)
        if [ -z "$RESPONSE" ]; then
            echo "  attempt $i: RPC unreachable, retrying"
            sleep "$SLEEP_BETWEEN"
            continue
        fi
        PENDING=$(echo "$RESPONSE" | jq -r '.result | length' 2>/dev/null || echo "?")
        if [ "$PENDING" = "0" ]; then
            echo "  pool drained after ${i} poll(s)"
            sleep "$FINAL_BUFFER"
            echo "drain-pool: done"
            exit 0
        fi
        echo "  attempt $i: $PENDING tx still pending"
        sleep "$SLEEP_BETWEEN"
    done
    echo "drain-pool: WARNING - pool not empty after $((MAX_ITERS * SLEEP_BETWEEN))s, proceeding anyway"
    sleep "$FINAL_BUFFER"

# Drain the parachain pool, then run another recipe. Use in CI between
# back-to-back integration tests to avoid stale-nonce drops.
# Usage: just drain-tx-pool-then demo "http://127.0.0.1:3334" "//Charlie" "//Dave"
[positional-arguments]
drain-tx-pool-then RECIPE *ARGS: drain-pool
    #!/usr/bin/env bash
    set -euo pipefail
    just "$@"

# Install workspace deps; descriptors regenerate from the tracked metadata snapshot
papi-setup:
    pnpm install

# ============================================================
# PAPI standalone demos (not covered by E2E suite)
# ============================================================

# Marketplace-style read-only walk of the Providers storage map
papi-provider-discovery BYTES="1073741824" DURATION="100" MAX_PRICE="10": papi-setup
    node --import tsx examples/papi/provider-discovery.ts "{{ CHAIN_WS }}" "{{ BYTES }}" "{{ DURATION }}" "{{ MAX_PRICE }}"

# Missed checkpoint slashing flow: configure_checkpoint_window (tight) ->
# wait past window -> report_missed_checkpoint (slashes leader, pays reporter).
papi-checkpoint-missed PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/checkpoint-missed.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# ============================================================
# E2E Test Suite
# ============================================================

# Run comprehensive E2E test suite (all 10 workflows sequentially)
e2e PROVIDER_URL=PROVIDER_URL: papi-setup
    npx c8 --reporter=text --reporter=json --report-dir=examples/papi/coverage \
        --include="examples/papi/**" --include="packages/sdk/src/**" \
        node --import tsx examples/papi/e2e/runner.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}"

# Run a single E2E workflow by number (e.g. just e2e-single 01)
e2e-single NUM PROVIDER_URL=PROVIDER_URL: papi-setup
    #!/usr/bin/env bash
    set -euo pipefail
    FILE=$(ls examples/papi/e2e/{{ NUM }}-*.ts 2>/dev/null | head -1)
    if [ -z "$FILE" ]; then
        echo "No workflow file matching examples/papi/e2e/{{ NUM }}-*.ts"
        exit 1
    fi
    node --import tsx "$FILE" "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}"

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
# ─── UI Tests ─────────────────────────────────────────────────────────────────
#
# Unit tests + Playwright e2e for drive-ui and provider.
# Requires a running local chain + provider node.

# Run all UI unit tests (Vitest)
test-ui-unit:
    pnpm run test:unit

# Run drive-ui Playwright e2e (requires chain + provider running)
test-ui-drive:
    pnpm run test:e2e:drive-ui

# Run provider Playwright e2e (requires chain running)
test-ui-provider:
    pnpm run test:e2e:provider

# Run ALL UI tests: unit + e2e for every UI. Assumes chain + provider already
# started (via `just start-chain` and `just start-provider` in separate
# terminals). The recipe waits for chain block #3 and provider /health before
# kicking off Playwright, then runs them serially per-UI.
test-ui:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "=== Probing local chain at {{CHAIN_WS}} ==="
    until curl -sf -X POST -H "Content-Type: application/json" \
        --data '{"jsonrpc":"2.0","method":"chain_getHeader","params":[],"id":1}' \
        http://127.0.0.1:{{CHAIN_PORT}} >/dev/null 2>&1; do
        echo "  waiting for chain..."
        sleep 2
    done

    echo "=== Probing provider /health at {{PROVIDER_URL}} ==="
    until curl -sf {{PROVIDER_URL}}/health >/dev/null 2>&1; do
        echo "  waiting for provider..."
        sleep 2
    done

    echo "=== Running unit tests ==="
    just test-ui-unit

    echo "=== drive-ui e2e ==="
    just test-ui-drive

    echo "=== provider e2e ==="
    just test-ui-provider

    echo "✅ All UI tests passed"
