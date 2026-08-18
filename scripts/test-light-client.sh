#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Manual e2e check for the provider node's embedded light-client transport.
#
# Starts the provider with --chain-transport light against an already-running
# local zombienet (relay + parachain, e.g. `just start-chain`), using the
# relay's raw spec from zombienet's network directory as-is, and a minimal
# parachain spec: the live spec's boot nodes plus a stateRootHash-only genesis
# (smoldot never executes para genesis — it derives the head from the relay —
# so the genesis state root alone identifies the chain). This is the same spec
# shape a production deployment would vet and ship.
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
# `ls` exits non-zero when one of the globs has no match (it still prints the
# matches of the other); tolerate that or set -e kills the script here.
ZOMBIE_DIR="${ZOMBIE_DIR:-$(ls -dt /var/folders/*/*/T/zombie-* /tmp/zombie-* 2>/dev/null | head -1 || true)}"

if [ -z "$ZOMBIE_DIR" ] || [ ! -d "$ZOMBIE_DIR" ]; then
    echo "No zombienet network directory found — is 'just start-chain' running?"
    exit 1
fi
echo "Using zombienet network dir: $ZOMBIE_DIR"

RELAY_SPEC=$(ls "$ZOMBIE_DIR"/*-local.json 2>/dev/null | grep -v '^.*/[0-9]' | head -1 || true)
PARA_LIVE=$(ls "$ZOMBIE_DIR"/[0-9]*-local.json 2>/dev/null | grep -v raw | grep -v plain | head -1 || true)
if [ -z "$RELAY_SPEC" ] || [ -z "$PARA_LIVE" ]; then
    echo "Could not locate chain specs in $ZOMBIE_DIR"
    exit 1
fi

# Genesis state root from the running para node. Fetching it trusts that node
# — fine for a dev script (the FetchFromRpc spec path has the same property);
# a production spec pins a vetted value here instead.
PARA_RPC="${PARA_RPC:-http://127.0.0.1:2222}"
rpc_result() {
    curl -sf -H 'Content-Type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" "$PARA_RPC" \
        | python3 -c "import json,sys;print(json.load(sys.stdin)['result']$3)"
}
GENESIS_HASH=$(rpc_result chain_getBlockHash '[0]' '')
STATE_ROOT=$(rpc_result chain_getHeader "[\"$GENESIS_HASH\"]" "['stateRoot']")

# Para spec for smoldot: the live spec's identity and boot nodes with the
# genesis replaced by its state root.
PARA_SPEC=$(mktemp "${TMPDIR:-/tmp}/para-light-spec.XXXXXX.json")
python3 - "$PARA_LIVE" "$RELAY_SPEC" "$STATE_ROOT" "$PARA_SPEC" <<'EOF'
import json, sys
live = json.load(open(sys.argv[1]))
relay = json.load(open(sys.argv[2]))
assert live.get("bootNodes"), "no boot nodes found in the live parachain spec"
# smoldot matches a parachain to its relay by exact id; zombienet's para spec
# says e.g. "westend-local" while the relay spec's id is "westend_local_testnet".
live["relay_chain"] = relay["id"]
live["genesis"] = {"stateRootHash": sys.argv[3]}
json.dump(live, open(sys.argv[4], "w"))
EOF
echo "Relay spec: $RELAY_SPEC"
echo "Para spec:  $PARA_SPEC (genesis stateRootHash $STATE_ROOT + $(python3 -c "import json,sys;print(len(json.load(open('$PARA_SPEC'))['bootNodes']))") boot node(s))"

KEYFILE=$(mktemp)
echo "//Alice" > "$KEYFILE" && chmod 600 "$KEYFILE"
LOG=$(mktemp "${TMPDIR:-/tmp}/light-client-provider.XXXXXX")

cleanup() {
    [ -n "${PROVIDER_PID:-}" ] && kill "$PROVIDER_PID" 2>/dev/null || true
    rm -f "$KEYFILE"
}
trap cleanup EXIT

echo "Starting provider with the embedded light client (log: $LOG)..."
RUST_LOG=info "$BIN" \
    --keyfile "$KEYFILE" --storage-mode inmemory \
    --bind-addr "0.0.0.0:$PROVIDER_PORT" \
    --chain-transport light \
    --relay-chain-spec "$RELAY_SPEC" \
    --para-chain-spec "$PARA_SPEC" > "$LOG" 2>&1 &
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
