# Quick Start Guide

This guide will help you test the system in under 5 minutes using `just` commands.

## Prerequisites

```bash
# Install just (if not already installed)
cargo install just
# Or on macOS:
# brew install just

# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## One-Time Setup

This command downloads all required binaries (polkadot, polkadot-omni-node, zombienet) and builds the project:

```bash
cd /Users/naren/DevBox/scalable-web3-storage-dev/web3-storage
just setup
```

**What it does:**
- ✅ Downloads polkadot binaries (polkadot, workers) to `.bin/`
- ✅ Downloads polkadot-omni-node to `.bin/`
- ✅ Downloads zombienet to `.bin/`
- ✅ Downloads chain-spec-builder to `.bin/`
- ✅ Builds the entire project in release mode

**Expected output:**
```
Downloading binaries...
All binaries downloaded to .bin/
Building project...
Finished `release` profile [optimized]
Setup complete! Run 'just start-chain' and 'just start-provider' to start the local network.
```

## Testing Steps

### Start Services

**Terminal 1: Start Blockchain Network**

```bash
just start-chain
```

**⚠️ Keep this terminal running!** Wait 30-60 seconds for initialization.

**Terminal 2: Start Provider Node**

Once blockchain is ready:

```bash
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
export CHAIN_RPC=ws://127.0.0.1:2222
cargo run --release -p storage-provider-node
```

---

### Verify Everything is Running

**Terminal 3: Check Services**

```bash
# Check blockchain
bash scripts/check-chain.sh

# Check provider health
just health
# Or:
curl http://localhost:3333/health
```

**Expected output:**
```json
{
  "status": "ok",
  "provider": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
}
```

---

### Run Quick Tests

```bash
just demo
```

---

## What the Tests Will Show

The quick test will fail at the upload step because:
1. ❌ Bucket doesn't exist on-chain
2. ❌ Provider not registered on-chain
3. ❌ No storage agreement between bucket and provider

This is **expected** - these require on-chain transactions.

---

## Complete Workflow (Manual Steps Required)

For a full end-to-end test, you need to:

### 1. Register Provider On-Chain

Open in browser: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222

**Step 1a: Register Provider (Basic Info)**

1. Go to **Developer > Extrinsics**
2. Select account: **ALICE**
3. Select pallet: **storageProvider**
4. Select extrinsic: **registerProvider**
5. Fill in parameters:
   - `multiaddr`: `/ip4/127.0.0.1/tcp/3333` (Polkadot.js will encode it)
   - `publicKey`: `0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d` (Alice's public key)
   - `stake`: `1000000000000000` (1000 tokens - minimum required stake)
6. Submit transaction

**Step 1b: Configure Provider Settings (Optional but Recommended)**

1. Same account: **ALICE**
2. Select extrinsic: **updateProviderSettings**
3. Fill in settings:
   - `minDuration`: `100` (minimum blocks)
   - `maxDuration`: `1000` (maximum blocks)
   - `pricePerByte`: `1000000` (price per byte per block)
   - `acceptingPrimary`: `true`
   - `replicaSyncPrice`: `Some(5000000)` or `None`
   - `acceptingExtensions`: `true`
4. Submit transaction

### 2. Create Bucket

1. Go to **Developer > Extrinsics**
2. Account: **ALICE**
3. Extrinsic: **storageProvider.createBucket**
4. Parameters:
   - `minProviders`: `1`
5. Submit

### 3. Create Storage Agreement

1. Account: **ALICE**
2. Extrinsic: **storageProvider.requestPrimaryAgreement**
3. Parameters:
   - `bucketId`: `0`
   - `provider`: ALICE (5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY)
   - `maxBytes`: `1073741824` (1 GB)
   - `duration`: `500` (blocks)
   - `maxPayment`: `600000000000000000` (maximum payment willing to pay - with 10% buffer)
4. Submit

**Payment Calculation:**
```
payment = price_per_byte × max_bytes × duration
payment = 1,000,000 × 1,073,741,824 × 500
payment = 536,870,912,000,000,000
```

The `maxPayment` must be ≥ calculated payment. Using `600000000000000000` gives ~12% buffer.

### 4. Accept Agreement (as Provider)

1. Account: **ALICE** (acting as provider)
2. Extrinsic: **storageProvider.acceptAgreement**
3. Parameters:
   - `bucketId`: `0`
4. Submit

### 5. Now Run Quick Tests Again

```bash
just demo
```

This time all tests should pass! ✅

---

## Alternative: Automated Setup Script

I can create a script that automates steps 1-4 using the Substrate CLI, but it requires:
- Installing `subxt-cli`
- Generating chain metadata
- Creating signed transactions programmatically

Would you like me to create this automated setup script?

---

## Full Testing Guide

For comprehensive testing including:
- Multiple providers
- Replica synchronization
- Challenge/slashing mechanism
- Performance benchmarks

See: [Manual Testing Guide](../testing/MANUAL_TESTING_GUIDE.md)

---

## Quick Reference

| Component | Address | Purpose |
|-----------|---------|---------|
| Relay Chain | ws://127.0.0.1:9900 | Validator network |
| Parachain | ws://127.0.0.1:2222 | Storage pallet chain |
| Provider HTTP | http://127.0.0.1:3333 | Data upload/download |
| Block Explorer | https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222 | UI for chain interaction |

---

## Troubleshooting

### "Chain is not responding"
- Ensure zombienet is running in Terminal 1
- Wait 30-60 seconds after starting
- Run `bash scripts/check-chain.sh` to verify

### "Provider node is not responding"
- Ensure provider is running in Terminal 2
- Check no other process is using port 3333
- Verify `CHAIN_RPC` environment variable is set

### "Insufficient stake" error when registering provider
- **Minimum stake required: 1000 tokens**
- Use: `1000000000000000` (not `100000000000`)
- The runtime uses 12 decimals: 1 token = 1,000,000,000,000
- Check Alice's balance in Accounts tab (should have millions on local testnet)

### "Upload failed with no agreement"
- Complete steps 1-4 in Polkadot.js UI first
- Verify agreement exists: Developer > Chain State > storageProvider.agreements(0, ALICE)

### "Provider not accepting agreements"
- Make sure you called `updateProviderSettings` after registration
- Check `acceptingPrimary` is set to `true` in provider settings

### "PaymentExceedsMax" error when creating agreement
- You're missing the `maxPayment` parameter or set it too low
- Calculate required payment: `price_per_byte × max_bytes × duration`
- Example: `1,000,000 × 1,073,741,824 × 500 = 536,870,912,000,000,000`
- Set `maxPayment` with 10-20% buffer: `600000000000000000`
- See "Create Storage Agreement" section for details

---

## Next Steps

Once the basic test passes:
1. ✅ Upload works
2. ✅ Download works
3. ✅ Data integrity verified

Try:
- Adding more providers
- Creating more buckets
- Testing challenges
- Benchmarking performance
