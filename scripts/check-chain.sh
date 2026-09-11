#!/bin/bash
#
# Quick health probe for a locally-running zombienet network.
#
# Hits system_health on the relay chain (port 9900) and parachain (port 2222),
# then prints the parachain's current block number.
#
# Use this when `just start-chain` is running to confirm both nodes are alive
# and the parachain is producing blocks. For provider node health, use `just health`.

echo "Checking blockchain status..."
echo ""

# Check relay chain
echo "Relay chain (port 9900):"
curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:9900 | jq .

echo ""

# Check parachain
echo "Parachain (port 2222):"
curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:2222 | jq .

echo ""

# Get block number
echo "Current block number:"
curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"chain_getHeader"}' \
  http://127.0.0.1:2222 | jq -r '.result.number' | xargs printf "Block: %d\n" 2>/dev/null || echo "Waiting for blocks..."
