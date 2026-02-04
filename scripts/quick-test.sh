#!/bin/bash
# Quick test script for basic functionality
# This assumes blockchain and provider nodes are already running

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

PARACHAIN_RPC="http://127.0.0.1:9944"
PROVIDER_URL="http://localhost:3000"

echo -e "${BLUE}=== Quick Test Suite ===${NC}"
echo ""

# Test 1: Check chain is running
echo -e "${YELLOW}Test 1: Checking blockchain connectivity...${NC}"
if curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  $PARACHAIN_RPC > /dev/null; then
    echo -e "${GREEN}✅ Chain is running${NC}"
else
    echo -e "${RED}❌ Chain is not responding${NC}"
    echo "Make sure zombienet is running: zombienet spawn zombienet.toml"
    exit 1
fi

# Test 2: Check provider node
echo ""
echo -e "${YELLOW}Test 2: Checking provider node...${NC}"
if curl -s $PROVIDER_URL/health > /dev/null; then
    echo -e "${GREEN}✅ Provider node is running${NC}"
    curl -s $PROVIDER_URL/health | jq .
else
    echo -e "${RED}❌ Provider node is not responding${NC}"
    echo "Make sure provider node is running on port 3000"
    exit 1
fi

# Test 3: Check chain state
echo ""
echo -e "${YELLOW}Test 3: Checking chain state...${NC}"
BLOCK_NUMBER=$(curl -s -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"chain_getHeader"}' \
  $PARACHAIN_RPC | jq -r '.result.number')

if [ "$BLOCK_NUMBER" != "null" ] && [ -n "$BLOCK_NUMBER" ]; then
    BLOCK_NUM_DEC=$((16#${BLOCK_NUMBER:2}))
    echo -e "${GREEN}✅ Chain is producing blocks (current: $BLOCK_NUM_DEC)${NC}"
else
    echo -e "${RED}❌ Unable to get block number${NC}"
    exit 1
fi

# Test 4: Upload test data
echo ""
echo -e "${YELLOW}Test 4: Uploading test data...${NC}"
TEST_DATA="Hello, Web3 Storage! $(date)"
echo "$TEST_DATA" > /tmp/quick-test.txt

UPLOAD_RESPONSE=$(curl -s -X POST $PROVIDER_URL/upload \
  -H "Content-Type: application/octet-stream" \
  -H "X-Bucket-Id: 0" \
  --data-binary @/tmp/quick-test.txt)

if echo "$UPLOAD_RESPONSE" | jq -e '.seq' > /dev/null 2>&1; then
    SEQ=$(echo "$UPLOAD_RESPONSE" | jq -r '.seq')
    echo -e "${GREEN}✅ Upload successful (seq: $SEQ)${NC}"
    echo "$UPLOAD_RESPONSE" | jq .
else
    echo -e "${RED}❌ Upload failed${NC}"
    echo "Response: $UPLOAD_RESPONSE"
    echo ""
    echo "Note: This might fail if bucket 0 doesn't exist or has no agreement."
    echo "Follow docs/testing/MANUAL_TESTING_GUIDE.md steps 7-8 to create bucket and agreement first."
    exit 1
fi

# Test 5: List bucket contents
echo ""
echo -e "${YELLOW}Test 5: Listing bucket contents...${NC}"
LIST_RESPONSE=$(curl -s $PROVIDER_URL/bucket/0/list)

if echo "$LIST_RESPONSE" | jq -e '.bucket_id' > /dev/null 2>&1; then
    LEAF_COUNT=$(echo "$LIST_RESPONSE" | jq '.leaves | length')
    echo -e "${GREEN}✅ Bucket listing successful ($LEAF_COUNT leaves)${NC}"
    echo "$LIST_RESPONSE" | jq '.leaves | .[:3]'
else
    echo -e "${YELLOW}⚠️  Bucket listing failed or bucket is empty${NC}"
    echo "Response: $LIST_RESPONSE"
fi

# Test 6: Download and verify
echo ""
echo -e "${YELLOW}Test 6: Downloading and verifying data...${NC}"
curl -s $PROVIDER_URL/bucket/0/download/$SEQ -o /tmp/downloaded-test.txt

if diff /tmp/quick-test.txt /tmp/downloaded-test.txt > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Download successful and data matches${NC}"
else
    echo -e "${RED}❌ Downloaded data doesn't match original${NC}"
    echo "Original:"
    cat /tmp/quick-test.txt
    echo "Downloaded:"
    cat /tmp/downloaded-test.txt
    exit 1
fi

# Test 7: Get MMR root
echo ""
echo -e "${YELLOW}Test 7: Getting MMR root...${NC}"
MMR_RESPONSE=$(curl -s $PROVIDER_URL/bucket/0/mmr-root)

if echo "$MMR_RESPONSE" | jq -e '.mmr_root' > /dev/null 2>&1; then
    echo -e "${GREEN}✅ MMR root retrieved${NC}"
    echo "$MMR_RESPONSE" | jq .
else
    echo -e "${YELLOW}⚠️  MMR root retrieval failed${NC}"
    echo "Response: $MMR_RESPONSE"
fi

# Cleanup
rm -f /tmp/quick-test.txt /tmp/downloaded-test.txt

echo ""
echo -e "${BLUE}=== Test Summary ===${NC}"
echo -e "${GREEN}✅ All basic tests passed!${NC}"
echo ""
echo "Next steps:"
echo "1. Create more buckets and agreements via Polkadot.js UI"
echo "2. Test with multiple providers"
echo "3. Test challenge and slashing mechanism"
echo "4. See docs/testing/MANUAL_TESTING_GUIDE.md for complete testing workflow"
