# Manual Testing Guide - Scalable Web3 Storage

This guide provides step-by-step commands to manually test the entire system from scratch.

## Quick Commands Summary

```bash
# One-time setup (downloads binaries + builds)
just setup

# Start services
just start-chain              # Terminal 1
just start-provider           # Terminal 2
just health                   # Check provider health

# Check prerequisites
just check

# Build project
just build

# List all available commands
just --list
```

## Prerequisites Setup

### 1. Install Required Tools

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Install just (command runner)
cargo install just
# Or on macOS:
# brew install just
```

### 2. One-Time Setup (Downloads Everything Automatically)

```bash
cd /Users/naren/DevBox/scalable-web3-storage-dev/web3-storage
just setup
```

**What this does:**
- ✅ Downloads polkadot binaries (polkadot + workers) to `.bin/`
- ✅ Downloads polkadot-omni-node to `.bin/`
- ✅ Downloads zombienet to `.bin/`
- ✅ Downloads chain-spec-builder to `.bin/`
- ✅ Builds the entire project in release mode

**Expected Output:**
```
Downloading polkadot for darwin/arm64...
polkadot downloaded to .bin/polkadot
...
All binaries downloaded to .bin/
Building project...
Finished `release` profile [optimized] target(s)
Setup complete! Run 'just start-chain' and 'just start-provider' to start the local network.
```

### 3. Verify Prerequisites

```bash
just check
```

**Expected Output:**
```
Checking prerequisites...
All prerequisites found!
```

---

## Step 1: Build the Project

Already done by `just setup`! If you need to rebuild:

```bash
just build
# Or:
cargo build --release
```

**Expected Output:**
```
Finished `release` profile [optimized] target(s) in X.XXs
```

### Verify build artifacts

```bash
# Check runtime WASM was built
ls -lh target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm

# Check provider node binary
ls -lh target/release/storage-provider-node
```

**Expected Output:**
- Runtime WASM file (~2-3 MB)
- Provider node binary exists

---

## Step 2: Start the Blockchain Network

### Start Services

**Terminal 1 - Start Zombienet**

```bash
just start-chain
```

**Expected Output:**
```
┌─────────────────────────────────────────┐
│         Network launched 🚀             │
├─────────────────────────────────────────┤
│ Namespace: zombie-xxx                   │
│ Provider: native                        │
└─────────────────────────────────────────┘

🪲  relay-chain node alice started
🪲  relay-chain node bob started
🪲  parachain 4000 collator storage-collator started
✅ Parachain 4000 onboarded
```

### Verify Blockchain is Running

```bash
# In another terminal
# Check relay chain
curl -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:9900

# Check parachain
curl -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:9944
```

**Expected Output:**
```json
{"jsonrpc":"2.0","result":{"peers":2,"isSyncing":false,"shouldHavePeers":true},"id":1}
```

### Access Block Explorer

Open in browser:
- **Relay Chain**: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900
- **Parachain**: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944

**Verify in UI:**
1. Connection indicator should be green
2. Block number should be increasing
3. Navigate to Developer > Chain State > storageProvider
4. Verify pallet is loaded (you should see: providers, buckets, agreements, etc.)

---

## Step 3: Fund Test Accounts

The well-known test accounts are pre-funded on local testnets:
- Alice: `5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY`
- Bob: `5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty`
- Charlie: `5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y`

### Verify Balances

```bash
# Check Alice's balance using RPC
curl -H "Content-Type: application/json" \
  -d '{
    "id":1,
    "jsonrpc":"2.0",
    "method":"system_accountNextIndex",
    "params":["5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"]
  }' \
  http://127.0.0.1:9944
```

**Or via Polkadot.js UI:**
1. Go to Accounts tab
2. See Alice, Bob, Charlie with large balances

---

## Step 4: Register Storage Providers On-Chain

### Register Provider via Polkadot.js UI

Navigate to: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944

**Step 4a: Register Provider (Basic Registration)**

1. Go to **Developer > Extrinsics**
2. Select account: **ALICE**
3. Select pallet: **storageProvider**
4. Select extrinsic: **registerProvider**
5. Fill parameters:
   - `multiaddr`: `/ip4/127.0.0.1/tcp/3000` (Polkadot.js handles encoding)
   - `publicKey`: `0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d`
     - This is Alice's Sr25519 public key (32 bytes)
   - `stake`: `1000000000000000` (1000 tokens = minimum required stake)
     - Runtime uses 12 decimals: 1 token = 1_000_000_000_000
     - Minimum stake = 1000 tokens = 1,000,000,000,000,000
6. Click **Submit Transaction**
7. Sign with Alice

**Step 4b: Update Provider Settings (Configure Pricing & Availability)**

1. Same account: **ALICE**
2. Select extrinsic: **updateProviderSettings**
3. Fill parameters:
   - `settings`:
     - `minDuration`: `100` (minimum agreement duration in blocks)
     - `maxDuration`: `10000` (maximum agreement duration in blocks)
     - `pricePerByte`: `1000000` (1 microtoken per byte per block)
     - `acceptingPrimary`: `true` (accepting new primary agreements)
     - `replicaSyncPrice`: `Some(5000000)` (5 microtokens per sync) or `None`
     - `acceptingExtensions`: `true` (accepting agreement extensions)
4. Submit transaction

**Note:** The default settings after registration have:
- `minDuration`: 0
- `maxDuration`: max block number
- `pricePerByte`: 0 (free!)
- `acceptingPrimary`: false
- `replicaSyncPrice`: None
- `acceptingExtensions`: false

So you **must** update settings to actually accept agreements!

### Verify Provider Registration

**Via Polkadot.js UI:**
1. Go to **Developer > Chain State**
2. Select pallet: **storageProvider**
3. Select query: **providers(AccountId): Option<ProviderInfo>**
4. Input: Alice's account ID: `5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY`
5. Click **Query**

**Expected Output:**
```json
{
  "multiaddr": "0x2f6970342f3132372e302e302e312f7463702f33303030",
  "publicKey": "0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d",
  "stake": "100,000,000,000",
  "committedBytes": 0,
  "settings": {
    "minDuration": 100,
    "maxDuration": 10000,
    "pricePerByte": "1,000,000",
    "acceptingPrimary": true,
    "replicaSyncPrice": "5,000,000",
    "acceptingExtensions": true
  },
  "stats": {
    "registeredAt": 42,
    "agreementsTotal": 0,
    "agreementsExtended": 0,
    "agreementsNotExtended": 0,
    "agreementsBurned": 0,
    "challengesReceived": 0,
    "challengesFailed": 0
  }
}
```

### Register Multiple Providers

Repeat steps 4a and 4b for **Bob** and **Charlie** to have 3 providers for testing:

- **Bob**: `5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty`
  - Public key: `0x8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48`
  - Use port 3001: `/ip4/127.0.0.1/tcp/3001`

- **Charlie**: `5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y`
  - Public key: `0x90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22`
  - Use port 3002: `/ip4/127.0.0.1/tcp/3002`

---

## Step 5: Start Provider Nodes Off-Chain

### Terminal 3 - Start Alice's Provider Node

```bash
cd /Users/naren/DevBox/scalable-web3-storage-dev/web3-storage

# Set environment variables
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
export CHAIN_RPC=ws://127.0.0.1:9944
export STORAGE_PATH=/tmp/provider-alice
export HTTP_PORT=3000

# Start provider node
cargo run --release -p storage-provider-node
```

**Expected Output:**
```
Storage Provider Node starting...
Provider ID: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
Chain RPC: ws://127.0.0.1:9944
HTTP API listening on: http://0.0.0.0:3000
Storage path: /tmp/provider-alice
```

### Terminal 4 - Start Bob's Provider Node

```bash
export PROVIDER_ID=5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
export CHAIN_RPC=ws://127.0.0.1:9944
export STORAGE_PATH=/tmp/provider-bob
export HTTP_PORT=3001

cargo run --release -p storage-provider-node
```

### Verify Provider Nodes

```bash
# Check Alice's provider (port 3000)
just health
# Or:
curl http://localhost:3000/health | jq .

# Check Bob's provider (port 3001)
curl http://localhost:3001/health | jq .
```

**Expected Output:**
```json
{
  "status": "ok",
  "provider": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
}
```

---

## Step 6: Test Using the Client SDK

### Terminal 5 - Create Test Script

```bash
cd /Users/naren/DevBox/scalable-web3-storage-dev/web3-storage
```

Create a test file `test_integration.rs` in `client/examples/`:

```rust
use storage_client::{StorageUserClient, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize client
    let config = ClientConfig {
        provider_urls: vec![
            "http://localhost:3000".to_string(),
            "http://localhost:3001".to_string(),
        ],
        chain_ws_url: "ws://127.0.0.1:9944".to_string(),
    };

    let mut client = StorageUserClient::new(config);
    client.connect_chain().await?;

    println!("✅ Connected to chain");

    // TODO: Add more test operations

    Ok(())
}
```

### Run SDK Test

```bash
cargo run --release --example test_integration
```

**Expected Output:**
```
✅ Connected to chain
```

---

## Step 7: Create a Bucket

### Via Polkadot.js UI

1. Go to **Developer > Extrinsics**
2. Select account: **ALICE** (bucket owner)
3. Select pallet: **storageProvider**
4. Select extrinsic: **createBucket**
5. Parameters:
   - `minProviders`: `2` (require 2 providers for redundancy)
6. **Submit Transaction**

### Verify Bucket Creation

1. Go to **Developer > Chain State**
2. Query: **storageProvider.buckets(u64): Option<Bucket>**
3. Input: `0` (first bucket ID)

**Expected Output:**
```json
{
  "members": [
    {"account": "5GrwvaEF...", "role": "Admin"}
  ],
  "frozenStartSeq": null,
  "minProviders": 2,
  "primaryProviders": [],
  "snapshot": null,
  "totalSnapshots": 0
}
```

---

## Step 8: Create Storage Agreements

### Request Primary Agreement

1. **Developer > Extrinsics**
2. Account: **ALICE** (bucket owner)
3. Pallet: **storageProvider**
4. Extrinsic: **requestPrimaryAgreement**
5. Parameters:
   - `bucketId`: `0`
   - `provider`: Alice's provider account
   - `maxBytes`: `1073741824` (1 GB)
   - `duration`: `500` (blocks)
   - `maxPayment`: `600000000000000000` (max payment willing to pay)

**Understanding maxPayment:**

This is a safety parameter. The actual payment is calculated as:
```
payment = price_per_byte × max_bytes × duration
payment = 1,000,000 × 1,073,741,824 × 500
payment = 536,870,912,000,000,000
```

Your `maxPayment` must be ≥ this calculated value. Adding 10-20% buffer is recommended:
- Calculated: `536,870,912,000,000,000`
- With 12% buffer: `600,000,000,000,000,000` ✅

If you set `maxPayment` too low, you'll get `PaymentExceedsMax` error.

### Provider Accepts Agreement

1. Account: **ALICE** (provider account)
2. Extrinsic: **acceptAgreement**
3. Parameters:
   - `bucketId`: `0`

### Verify Agreement

Query: **storageProvider.agreements(u64, AccountId): Option<Agreement>**
- Input: Bucket ID `0`, Provider: Alice's account

**Expected Output:**
```json
{
  "owner": "5GrwvaEF...",
  "maxBytes": "1,073,741,824",
  "paymentLocked": "536,870,912,000,000,000",
  "pricePerByte": "1,000,000",
  "expiresAt": 500,
  "role": "Primary",
  ...
}
```

---

## Step 9: Upload Data to Provider

### Upload via HTTP API

```bash
# Create test file
echo "Hello, decentralized storage!" > /tmp/test-data.txt

# Upload to Alice's provider
curl -X POST http://localhost:3000/upload \
  -H "Content-Type: application/octet-stream" \
  -H "X-Bucket-Id: 0" \
  --data-binary @/tmp/test-data.txt
```

**Expected Output:**
```json
{
  "seq": 0,
  "hash": "0x1234...",
  "size": 30
}
```

### Verify Upload

```bash
# List bucket contents
curl http://localhost:3000/bucket/0/list
```

**Expected Output:**
```json
{
  "bucket_id": 0,
  "leaves": [
    {
      "seq": 0,
      "hash": "0x1234...",
      "size": 30
    }
  ],
  "mmr_root": "0xabcd..."
}
```

---

## Step 10: Download Data from Provider

```bash
# Download by sequence number
curl http://localhost:3000/bucket/0/download/0 -o /tmp/downloaded.txt

# Verify content matches
diff /tmp/test-data.txt /tmp/downloaded.txt
```

**Expected Output:**
- No output from diff (files match)

---

## Step 11: Create Checkpoint (Snapshot)

### Get MMR Root and Signatures

```bash
# Get current MMR state
curl http://localhost:3000/bucket/0/mmr-root
```

**Response:**
```json
{
  "bucket_id": 0,
  "mmr_root": "0xabcd1234...",
  "start_seq": 0,
  "leaf_count": 1
}
```

### Submit Checkpoint On-Chain

1. **Developer > Extrinsics**
2. Account: **ALICE** (writer)
3. Extrinsic: **storageProvider.checkpoint**
4. Parameters:
   - `bucketId`: `0`
   - `mmrRoot`: `0xabcd1234...` (from above)
   - `startSeq`: `0`
   - `leafCount`: `1`
   - `signatures`: Array of (AccountId, Signature) tuples from providers

### Verify Checkpoint

Query: **storageProvider.buckets(0)**

**Expected Output:**
```json
{
  ...
  "snapshot": {
    "mmrRoot": "0xabcd1234...",
    "startSeq": 0,
    "leafCount": 1,
    "checkpointBlock": 150,
    "primarySigners": [1, 0]
  },
  "totalSnapshots": 1
}
```

---

## Step 12: Test Challenge Flow

### Submit Challenge

1. Account: **CHARLIE** (challenger, not a provider)
2. Extrinsic: **storageProvider.challenge**
3. Parameters:
   - `bucketId`: `0`
   - `provider`: Alice's account
   - `leafIndex`: `0`
   - `chunkIndex`: `0`

### Provider Responds to Challenge

```bash
# Provider node automatically detects challenge and responds
# Check logs in Terminal 3 (Alice's provider)
```

**Expected Log:**
```
Challenge received for bucket 0, leaf 0, chunk 0
Generating proof...
Submitting proof to chain...
Challenge resolved successfully
```

### Verify Challenge Resolution

Query: **storageProvider.challenges(BlockNumber): Vec<Challenge>**

Check that challenge is marked as resolved.

---

## Step 13: Test Slashing (Failed Challenge)

### Stop Provider Node

```bash
# In Terminal 3, press Ctrl+C to stop Alice's provider
```

### Submit Challenge While Provider is Down

1. Account: **CHARLIE**
2. Extrinsic: **storageProvider.challenge**
3. Wait for challenge timeout (configured as 100 blocks)

### Wait for Timeout

```bash
# Monitor block numbers in Polkadot.js UI
# After timeout passes (~10 minutes at 6s/block)
```

### Verify Slashing

1. Check Alice's provider stake is reduced
2. Query: **storageProvider.providers(Alice)**
3. Verify `stake` has decreased

---

## Step 14: Test Replica Sync

### Start Third Provider (Charlie)

```bash
# Terminal 6
export PROVIDER_ID=5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y
export CHAIN_RPC=ws://127.0.0.1:9944
export STORAGE_PATH=/tmp/provider-charlie
export HTTP_PORT=3002

cargo run --release -p storage-provider-node
```

### Create Replica Agreement

1. Account: **ALICE** (bucket owner)
2. Extrinsic: **storageProvider.requestReplicaAgreement**
3. Parameters:
   - `bucketId`: `0`
   - `provider`: Charlie's account
   - `primaryProvider`: Alice's account

### Trigger Sync

```bash
# Charlie's provider should automatically start syncing
# Check logs in Terminal 6
```

**Expected Log:**
```
Starting replica sync from primary provider
Fetching MMR peaks...
Downloading subtree...
Sync complete: 1 leaves synced
```

### Verify Replica Data

```bash
# Download from replica
curl http://localhost:3002/bucket/0/download/0 -o /tmp/replica-data.txt

# Verify matches original
diff /tmp/test-data.txt /tmp/replica-data.txt
```

---

## Step 15: Performance Testing

### Upload Multiple Files

```bash
# Create test script
for i in {1..100}; do
  echo "Test data $i" > /tmp/test-$i.txt
  curl -X POST http://localhost:3000/upload \
    -H "Content-Type: application/octet-stream" \
    -H "X-Bucket-Id: 0" \
    --data-binary @/tmp/test-$i.txt \
    -w "\n%{time_total}s\n"
done
```

### Measure Download Performance

```bash
# Benchmark downloads
for i in {1..100}; do
  curl http://localhost:3000/bucket/0/download/$i \
    -o /dev/null \
    -w "Download $i: %{time_total}s\n"
done
```

---

## Troubleshooting

### Issue: Zombienet won't start

**Solution:**
```bash
# Ensure polkadot binary is in PATH
which polkadot

# Or set explicit path
export POLKADOT_BINARY_PATH=/full/path/to/polkadot
```

### Issue: Provider node can't connect to chain

**Solution:**
```bash
# Verify chain is running
curl -H "Content-Type: application/json" \
  -d '{"id":1, "jsonrpc":"2.0", "method":"system_health"}' \
  http://127.0.0.1:9944

# Check firewall isn't blocking port 9944
```

### Issue: Upload fails with "no agreement"

**Solution:**
1. Verify agreement exists on-chain
2. Check agreement hasn't expired
3. Ensure provider has accepted the agreement

### Issue: Challenge proof verification fails

**Solution:**
1. Verify provider has the actual data
2. Check MMR state is consistent
3. Ensure checkpoint was properly created

---

## Cleanup

### Stop All Services

```bash
# Stop provider nodes (Ctrl+C in each terminal)
# Stop zombienet (Ctrl+C in Terminal 1)

# Clean up data directories
rm -rf /tmp/provider-alice
rm -rf /tmp/provider-bob
rm -rf /tmp/provider-charlie
rm -rf /tmp/test-*.txt
```

### Reset Chain State

```bash
# If using zombienet, just restart it
# Data is ephemeral by default
```

---

## Success Criteria

You have successfully tested the system if:

- ✅ All components build without errors
- ✅ Blockchain network starts and produces blocks
- ✅ Providers can register on-chain
- ✅ Provider nodes start and respond to health checks
- ✅ Buckets can be created with proper access control
- ✅ Storage agreements are created between owners and providers
- ✅ Data can be uploaded to providers via HTTP
- ✅ Data can be downloaded and matches original
- ✅ Checkpoints can be created with provider signatures
- ✅ Challenges can be submitted and responded to
- ✅ Failed challenges result in slashing
- ✅ Replica providers can sync data from primary providers

---

## Next Steps

1. **Mainnet Deployment**: Deploy to Rococo or Kusama testnet
2. **Production Hardening**: Add monitoring, metrics, and alerting
3. **Client SDKs**: Test integration with applications
4. **Load Testing**: Stress test with larger datasets
5. **Security Audit**: Professional security review before mainnet

---

## Additional Resources

- [Architecture Overview](./README.md)
- [Client SDK Documentation](./client/README.md)
- [Provider Node API](./provider-node/README.md)
- [Pallet API Documentation](./pallet/README.md)
