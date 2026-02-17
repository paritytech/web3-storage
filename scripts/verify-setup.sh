#!/bin/bash
# Verify on-chain setup for quick-test.sh

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PARACHAIN_RPC="http://127.0.0.1:2222"
ALICE_ACCOUNT="5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"

echo -e "${BLUE}=== Verifying On-Chain Setup ===${NC}"
echo ""

# Helper function to query chain state
query_chain() {
    local method=$1
    local params=$2
    curl -s -H "Content-Type: application/json" \
        -d "{\"id\":1, \"jsonrpc\":\"2.0\", \"method\":\"$method\", \"params\":$params}" \
        $PARACHAIN_RPC
}

# Check 1: Provider registered
echo -e "${YELLOW}Check 1: Is provider registered?${NC}"
PROVIDER_QUERY=$(query_chain "state_getStorage" "[\"0x$(echo -n 'StorageProvider' 'Providers' | xxd -p)\"]")
if echo "$PROVIDER_QUERY" | grep -q "result"; then
    echo -e "${GREEN}✅ Provider query works${NC}"
    echo "Note: Manual verification needed in Polkadot.js UI"
else
    echo -e "${RED}❌ Cannot query provider state${NC}"
fi

# Check 2: Bucket exists
echo ""
echo -e "${YELLOW}Check 2: Does bucket 0 exist?${NC}"
echo "Manual verification needed in Polkadot.js UI:"
echo "  1. Go to: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222"
echo "  2. Developer > Chain State"
echo "  3. Query: storageProvider.buckets(0)"
echo "  4. Should return bucket info (not None)"

# Check 3: Agreement exists
echo ""
echo -e "${YELLOW}Check 3: Does agreement exist for bucket 0?${NC}"
echo "Manual verification in Polkadot.js UI:"
echo "  1. Query: storageProvider.agreements(0, $ALICE_ACCOUNT)"
echo "  2. Should return agreement info (not None)"

echo ""
echo -e "${BLUE}=== Setup Instructions ===${NC}"
echo ""
echo "If any checks failed, complete these steps in Polkadot.js UI:"
echo ""
echo -e "${GREEN}Step 1: Register Provider${NC}"
echo "  Extrinsic: storageProvider.registerProvider"
echo "  Parameters:"
echo "    - multiaddr: /ip4/127.0.0.1/tcp/3333"
echo "    - publicKey: 0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d"
echo "    - stake: 1000000000000000"
echo ""
echo -e "${GREEN}Step 2: Update Provider Settings${NC}"
echo "  Extrinsic: storageProvider.updateProviderSettings"
echo "  Parameters:"
echo "    - minDuration: 100"
echo "    - maxDuration: 10000"
echo "    - pricePerByte: 1000000"
echo "    - acceptingPrimary: true"
echo "    - replicaSyncPrice: None"
echo "    - acceptingExtensions: true"
echo ""
echo -e "${GREEN}Step 3: Create Bucket${NC}"
echo "  Extrinsic: storageProvider.createBucket"
echo "  Parameters:"
echo "    - minProviders: 1"
echo ""
echo -e "${GREEN}Step 4: Request Agreement${NC}"
echo "  Extrinsic: storageProvider.requestPrimaryAgreement"
echo "  Parameters:"
echo "    - bucketId: 0"
echo "    - provider: $ALICE_ACCOUNT"
echo "    - maxBytes: 1073741824"
echo "    - duration: 500"
echo "    - maxPayment: 600000000000000000"
echo ""
echo -e "${GREEN}Step 5: Accept Agreement${NC}"
echo "  Extrinsic: storageProvider.acceptAgreement"
echo "  Parameters:"
echo "    - bucketId: 0"
echo ""
echo "After completing these steps, run: bash scripts/quick-test.sh"
