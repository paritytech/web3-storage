#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Assemble smoldot-ready chain spec files for the provider node's
# `--chain-transport light` from a running zombie-cli network.
#
# zombie-cli already writes the raw relay and parachain specs the nodes run
# with (<base_dir>/<node>/cfg/<chain>.json). Two things keep smoldot from
# using them as-is:
#   - `bootNodes` is empty: zombie-cli hands bootnodes to each node on the
#     command line (`--bootnodes`), never in the spec.
#   - the parachain spec's `relay_chain` carries the name the spec was built
#     with (`westend-local`) while smoldot matches it against the relay spec's
#     `id` (`westend_local_testnet`).
# So this fills `bootNodes` from each node's own listen addresses and peer id
# (system_localListenAddresses + system_localPeerId, the same data the
# provider's fetch path uses) and aligns `relay_chain`.
#
# Usage: scripts/light-client-specs.sh <zombie-cli base_dir> <out-dir>
#   base_dir: the `base_dir:` path zombie-cli logs at spawn (CI pins it with
#             `zombienet spawn -d`).
# Env: RELAY_RPC (http://127.0.0.1:9900), PARA_RPC (http://127.0.0.1:2222),
#      RELAY_CHAIN (westend-local), PARA_ID (1600) - match the zombienet TOML.
# Writes <out-dir>/relay.json and <out-dir>/para.json.

set -euo pipefail

BASE_DIR="${1:?usage: $0 <zombie-cli base_dir> <out-dir>}"
OUT_DIR="${2:?usage: $0 <zombie-cli base_dir> <out-dir>}"
RELAY_RPC="${RELAY_RPC:-http://127.0.0.1:9900}"
PARA_RPC="${PARA_RPC:-http://127.0.0.1:2222}"
RELAY_CHAIN="${RELAY_CHAIN:-westend-local}"
PARA_ID="${PARA_ID:-1600}"

# Every node's cfg/ copy of a spec is identical; take the first one found.
find_spec() {
    local found
    found=$(find "$BASE_DIR" -path "*/cfg/$1.json" 2>/dev/null | head -n 1)
    if [ -z "$found" ]; then
        echo "ERROR: no */cfg/$1.json under $BASE_DIR (is zombie-cli up, spawned with -d $BASE_DIR?)" >&2
        exit 1
    fi
    echo "$found"
}

rpc() {
    curl -sf -H 'Content-Type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[]}" "$1" \
        | jq -e '.result' \
        || { echo "ERROR: $2 on $1 failed (node down, or RPC not reachable over HTTP)" >&2; exit 1; }
}

# The node's dialable TCP addresses (raw TCP and WS both work for native
# smoldot) with its canonical peer id appended; mirrors boot_node_addrs() in
# crates/providers/chain/src/chain_connection.rs.
boot_nodes() {
    local peer
    peer=$(rpc "$1" system_localPeerId | jq -r .)
    rpc "$1" system_localListenAddresses \
        | jq -c --arg peer "$peer" \
            '[.[] | select(contains("/tcp/")) | split("/p2p/")[0] + "/p2p/" + $peer] | unique'
}

RELAY_SRC=$(find_spec "$RELAY_CHAIN")
PARA_SRC=$(find_spec "$PARA_ID")
RELAY_BOOT=$(boot_nodes "$RELAY_RPC")
PARA_BOOT=$(boot_nodes "$PARA_RPC")
[ "$RELAY_BOOT" != "[]" ] || { echo "ERROR: relay node at $RELAY_RPC reports no TCP listen addresses" >&2; exit 1; }
[ "$PARA_BOOT" != "[]" ] || { echo "ERROR: parachain node at $PARA_RPC reports no TCP listen addresses" >&2; exit 1; }

# zombie-cli writes some genesis values it patched in without the 0x prefix.
# Substrate's hex decoder accepts that; smoldot's chain-spec parser does not.
HEX0X='def hex0x: if startswith("0x") then . else "0x" + . end;
    .genesis.raw.top |= with_entries(.key |= hex0x | .value |= hex0x)'

mkdir -p "$OUT_DIR"
jq --argjson boot "$RELAY_BOOT" "$HEX0X | .bootNodes = \$boot" "$RELAY_SRC" > "$OUT_DIR/relay.json"
RELAY_ID=$(jq -r '.id' "$OUT_DIR/relay.json")
jq --arg relay "$RELAY_ID" --argjson boot "$PARA_BOOT" \
    "$HEX0X | .relay_chain = \$relay | .bootNodes = \$boot" "$PARA_SRC" > "$OUT_DIR/para.json"

echo "relay: $RELAY_SRC -> $OUT_DIR/relay.json"
echo "       id=$RELAY_ID bootNodes=$RELAY_BOOT"
echo "para:  $PARA_SRC -> $OUT_DIR/para.json"
echo "       id=$(jq -r '.id' "$OUT_DIR/para.json") relay_chain=$RELAY_ID bootNodes=$PARA_BOOT"
