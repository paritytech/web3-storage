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

# Install PAPI dependencies and generate chain descriptors (requires running chain)
papi-setup:
    #!/usr/bin/env bash
    set -euo pipefail
    cd examples/papi
    npm install
    npm run papi:generate

# ============================================================
# PAPI single-purpose demos
# ============================================================
# Each script exercises one pallet workflow via the typed PAPI client.
# All assume the chain (and the provider, for non-read-only ones) is running.

# Bucket ACL flow: create_bucket -> set_member -> promote -> remove_member (pure on-chain)
papi-bucket-membership ADMIN="//Alice" WRITER="//Eve" READER="//Ferdie": papi-setup
    node examples/papi/bucket-membership.js "{{ CHAIN_WS }}" "{{ ADMIN }}" "{{ WRITER }}" "{{ READER }}"

# Marketplace-style read-only walk of the Providers storage map
papi-provider-discovery BYTES="1073741824" DURATION="100" MAX_PRICE="10": papi-setup
    node examples/papi/provider-discovery.js "{{ CHAIN_WS }}" "{{ BYTES }}" "{{ DURATION }}" "{{ MAX_PRICE }}"

# Atomic create_bucket_with_storage -> upload -> checkpoint -> freeze_bucket
papi-bucket-with-storage PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node examples/papi/bucket-with-storage.js "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# S3 registry workflow: create_s3_bucket -> put/copy/delete object metadata -> delete_s3_bucket
papi-s3-lifecycle PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node examples/papi/s3-lifecycle.js "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Drive registry workflow: create_drive -> share -> unshare -> delete_drive
papi-drive-lifecycle PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" OWNER_SEED="//Bob" MEMBER_SEED="//Ferdie": papi-setup
    node examples/papi/drive-lifecycle.js "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ OWNER_SEED }}" "{{ MEMBER_SEED }}"

# Provider-initiated checkpoint + reward flow: configure_checkpoint_window ->
# fund_checkpoint_pool -> provider_checkpoint -> claim_checkpoint_rewards.
papi-checkpoint-rewards PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node examples/papi/checkpoint-rewards.js "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

# Missed checkpoint slashing flow: configure_checkpoint_window (tight) ->
# wait past window -> report_missed_checkpoint (slashes leader, pays reporter).
papi-checkpoint-missed PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node examples/papi/checkpoint-missed.js "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"

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
# Unit tests + Playwright e2e for drive-ui, console-ui, and provider.
# Requires a running local chain + provider node.

# Run all UI unit tests (Vitest)
test-ui-unit:
    cd user-interfaces && pnpm run test:unit

# Run drive-ui Playwright e2e (requires chain + provider running)
test-ui-drive:
    cd user-interfaces && pnpm run test:e2e:drive-ui

# Run console-ui Playwright e2e (requires chain running)
test-ui-console:
    cd user-interfaces && pnpm run test:e2e:console-ui

# Run provider Playwright e2e (requires chain running)
test-ui-provider:
    cd user-interfaces && pnpm run test:e2e:provider

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

    echo "=== console-ui e2e ==="
    just test-ui-console

    echo "=== provider e2e ==="
    just test-ui-provider

    echo "✅ All UI tests passed"
