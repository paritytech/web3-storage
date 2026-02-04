#!/bin/bash
# Check if blockchain is ready

echo "Checking blockchain status..."
echo ""

# Check relay chain
echo "Relay chain (port 9900):"
curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:9900 | jq .

echo ""

# Check parachain
echo "Parachain (port 9944):"
curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:9944 | jq .

echo ""

# Get block number
echo "Current block number:"
curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"chain_getHeader"}' \
  http://127.0.0.1:9944 | jq -r '.result.number' | xargs printf "Block: %d\n" 2>/dev/null || echo "Waiting for blocks..."
