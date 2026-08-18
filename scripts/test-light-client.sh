#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Manual e2e check for the provider node's embedded light-client transport.
#
# Starts the provider with --chain-transport light against an already-running
# local zombienet (relay + parachain, e.g. `just start-chain`), exercising the
# FetchFromRpc spec path end to end: the relay spec is fetched from the relay
# node's sync-state RPC (with the node's own address injected as boot node),
# and the parachain spec is assembled from ordinary RPC calls on --chain-rpc
# (genesis state root, boot-node addresses, para id).
# Waits until the provider's chain-state coordinator reports synced provider
# info on /info.
#
# Prerequisites:
#   - `just start-chain` running
#   - provider registered on-chain (e.g. `just register-provider`), otherwise
#     the check passes on /health + block-following alone
#   - `cargo build --release -p storage-provider-node`
#
# Usage: scripts/test-light-client.sh [timeout-seconds]

set -euo pipefail

PROVIDER_PORT="${PROVIDER_PORT:-3433}"
TIMEOUT="${1:-300}"
BIN="${BIN:-./target/release/storage-provider-node}"
RELAY_RPC="${RELAY_RPC:-ws://127.0.0.1:9900}"
PARA_RPC="${PARA_RPC:-ws://127.0.0.1:2222}"

KEYFILE=$(mktemp)
echo "//Alice" > "$KEYFILE" && chmod 600 "$KEYFILE"
LOG=$(mktemp "${TMPDIR:-/tmp}/light-client-provider.XXXXXX")
# Own throwaway storage: the RPC provider on 3333 holds ./provider-data.
STORAGE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/light-client-storage.XXXXXX")

cleanup() {
    [ -n "${PROVIDER_PID:-}" ] && kill "$PROVIDER_PID" 2>/dev/null || true
    rm -f "$KEYFILE"
    rm -rf "$STORAGE_DIR"
}
trap cleanup EXIT

echo "Starting provider with the embedded light client (log: $LOG)..."
echo "Specs fetched from: relay $RELAY_RPC, para $PARA_RPC"
RUST_LOG=info "$BIN" \
    --keyfile "$KEYFILE" --storage-path "$STORAGE_DIR" \
    --bind-addr "0.0.0.0:$PROVIDER_PORT" \
    --chain-rpc "$PARA_RPC" \
    --chain-transport light \
    --relay-chain-spec "$RELAY_RPC" > "$LOG" 2>&1 &
PROVIDER_PID=$!

echo "Waiting up to ${TIMEOUT}s for the light client to sync..."
DEADLINE=$(( $(date +%s) + TIMEOUT ))
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    if ! kill -0 "$PROVIDER_PID" 2>/dev/null; then
        echo "FAILED: provider exited early. Last log lines:"
        tail -20 "$LOG"
        exit 1
    fi
    if curl -sf "http://127.0.0.1:$PROVIDER_PORT/info" 2>/dev/null \
            | grep -q '"provider_registration_info":{'; then
        echo "PASSED: provider synced its on-chain registration via the light client."
        grep -E "light client|following finalized" "$LOG" | tail -3
        exit 0
    fi
    sleep 5
done

echo "FAILED: /info did not report synced provider info within ${TIMEOUT}s."
echo "Last log lines:"
tail -30 "$LOG"
exit 1
