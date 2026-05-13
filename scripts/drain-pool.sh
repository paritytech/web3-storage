#!/usr/bin/env bash
#
# Drain barrier for CI: wait until the parachain's transaction pool is empty,
# then sleep one block to let in-pool finalize. Run this between back-to-back
# integration-test steps so the next step doesn't pick up an `accountNextIndex`
# that misses an in-flight tx from the previous step (which would land with the
# same nonce and get dropped as "Usurped").
#
# Usage: scripts/drain-pool.sh [ws-or-http-rpc-url]
#   Defaults to ws://127.0.0.1:2222.
#
# Bounded: polls ~60s total, then proceeds anyway with a warning. We never want
# a stuck pool to deadlock CI — the next step's own checks will catch real
# breakage with a clearer error.

set -euo pipefail

RPC_INPUT="${1:-ws://127.0.0.1:2222}"
# author_pendingExtrinsics works over plain HTTP; convert if a ws:// URL was passed.
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
exit 0
