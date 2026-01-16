# Scalable Web3 Storage

A decentralized storage system built on Substrate with game-theoretic guarantees. Storage providers lock stake and face slashing for data loss, while the chain acts as a credible threat rather than the hot path.

## Overview

This system provides bucket-based storage where:
- **Providers** register with stake and offer storage services
- **Clients** create buckets to organize their data
- **Storage agreements** bind providers to store data for agreed durations
- **Challenges** enforce accountability through slashing

Normal operations (reads, writes, storage) happen off-chain between clients and providers. The chain is only touched for setup, checkpoints, and disputes.

## Design Documents

- [Scalable Web3 Storage](./docs/scalable-web3-storage.md) - High-level design and rationale
- [Implementation Details](./docs/scalable-web3-storage-implementation.md) - On-chain and off-chain interfaces

## Architecture

The system consists of two node types that work together:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              BLOCKCHAIN LAYER                               │
│                                                                             │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │              Polkadot Omni Node + storage-parachain-runtime           │  │
│  │                                                                       │  │
│  │  ┌─────────────────────────────────────────────────────────────────┐  │  │
│  │  │                    pallet-storage-provider                      │  │  │
│  │  │  • Provider registration & stake management                     │  │  │
│  │  │  • Bucket creation & membership                                 │  │  │
│  │  │  • Storage agreements (primary & replica)                       │  │  │
│  │  │  • Checkpoints & dispute resolution                             │  │  │
│  │  └─────────────────────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
│    Touched for: registration, agreements, checkpoints, disputes             │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
                                      ▲
                                      │ extrinsics (infrequent)
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              STORAGE LAYER                                  │
│                                                                             │
│   ┌─────────────┐                  ┌─────────────────────────────────────┐  │
│   │   Client    │   HTTP API       │         Provider Node               │  │
│   │  (storage-  │ ◄──────────────► │    (storage-provider-node)          │  │
│   │   client)   │  reads/writes    │                                     │  │
│   └─────────────┘                  │  • Content-addressed storage        │  │
│                                    │  • MMR commitments                  │  │
│                                    │  • Challenge responses              │  │
│                                    │  • Replica sync                     │  │
│                                    └─────────────────────────────────────┘  │
│                                                                             │
│    Hot path: all data reads/writes happen here (no blockchain)              │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Two Nodes, Two Purposes

| Node | Purpose | Run by |
|------|---------|--------|
| **Parachain Node** (Omni Node + Runtime) | Blockchain consensus, state transitions, finality | Collators (parachain validators) |
| **Provider Node** | Store actual data, serve clients, respond to challenges | Storage providers |

**Yes, you run both nodes** if you're a storage provider:
- The **parachain node** participates in the blockchain network
- The **provider node** handles actual storage operations

## Project Structure

```
scalable-web3-storage/
├── primitives/           # Shared types (BucketId, Role, MMR types, etc.)
├── pallet/               # Substrate pallet (on-chain logic)
├── runtime/              # Parachain runtime for Polkadot/Rococo
├── provider-node/        # Off-chain provider node (HTTP server)
├── client/               # Client library for storage operations
├── chain-specs/          # Chain specification files
└── docs/                 # Design documents
```

## Building

### Prerequisites

- Rust 1.74 or later
- Cargo

### Build all crates

```bash
cargo build --release
```

### Build individual crates

```bash
cargo build -p storage-primitives
cargo build -p pallet-storage-provider
cargo build -p storage-provider-node
cargo build -p storage-client
```

## Quick Start

This guide shows how to run a complete local development environment with both nodes.

### Prerequisites

- Rust 1.74+ with `wasm32-unknown-unknown` target
- [Zombienet](https://github.com/paritytech/zombienet) for local relay chain
- [Polkadot Omni Node](https://github.com/paritytech/polkadot-sdk) binary

```bash
# Install wasm target
rustup target add wasm32-unknown-unknown

# Install zombienet (option 1: cargo)
cargo install zombienet

# Or download from releases (option 2)
# https://github.com/paritytech/zombienet/releases
```

### Step 1: Build Everything

```bash
# Build all crates including the runtime
cargo build --release

# The runtime WASM will be at:
# target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm
```

### Step 2: Start the Blockchain (Parachain Node)

**Option A: Local Development with Zombienet**

Create `zombienet.toml`:
```toml
[relaychain]
default_command = "polkadot"
chain = "rococo-local"

  [[relaychain.nodes]]
  name = "alice"
  validator = true

  [[relaychain.nodes]]
  name = "bob"
  validator = true

[[parachains]]
id = 4000
chain_spec_path = "chain-specs/storage-rococo.json"

  [parachains.collator]
  name = "storage-collator"
  command = "polkadot-omni-node"
  args = ["--runtime", "target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm"]
```

Start the network:
```bash
zombienet spawn zombienet.toml
```

**Option B: Connect to Existing Network**

```bash
polkadot-omni-node \
  --collator \
  --chain chain-specs/storage-rococo.json \
  --runtime target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm \
  --relay-chain-rpc-urls wss://rococo-rpc.polkadot.io
```

### Step 3: Start the Provider Node (Storage Server)

In a new terminal:
```bash
# Set your provider's account (must match on-chain registered provider)
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY  # Alice

# Set the parachain RPC endpoint
export CHAIN_RPC=ws://127.0.0.1:9944

# Start the provider node
cargo run --release -p storage-provider-node

# Or with custom port
BIND_ADDR=0.0.0.0:8080 cargo run --release -p storage-provider-node
```

### Step 4: Verify Both Nodes are Running

```bash
# Check provider node health
curl http://localhost:3000/health
# {"status":"healthy","version":"0.1.0"}

# Check parachain via RPC (if using polkadot.js or similar)
# Connect to ws://127.0.0.1:9944
```

### Step 5: Use the Client

```bash
# In another terminal, run an example
cargo run --example upload_file -p storage-client
```

Or use the client library in your code:
```rust
use storage_client::StorageClient;

let client = StorageClient::new("http://localhost:3000");
let health = client.health().await?;
println!("Provider: {}", health.status);
```

## Testing

### Run all tests

```bash
cargo test
```

### Run tests for specific crates

```bash
# Pallet unit tests
cargo test -p pallet-storage-provider

# Provider node tests (unit + integration)
cargo test -p storage-provider-node

# Client tests (unit + integration)
cargo test -p storage-client

# Primitives tests
cargo test -p storage-primitives
```

### Run integration tests only

```bash
cargo test --test api_integration -p storage-provider-node
cargo test --test client_integration -p storage-client
```

## Running the Provider Node

The provider node is a separate process that handles actual data storage. It must be run alongside a parachain node (or connect to one via RPC).

### Start a provider node

```bash
# Minimum required: set your provider account
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY

# Start with defaults (port 3000, connects to local chain)
cargo run --release -p storage-provider-node

# Production example with all options
BIND_ADDR=0.0.0.0:8080 \
PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY \
CHAIN_RPC=ws://127.0.0.1:9944 \
DATA_DIR=/var/lib/storage-provider \
RUST_LOG=storage_provider_node=info \
cargo run --release -p storage-provider-node
```

### Environment variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BIND_ADDR` | Address to bind the HTTP server | `0.0.0.0:3000` |
| `PROVIDER_ID` | Provider's on-chain account ID (SS58) | Required |
| `CHAIN_RPC` | Parachain WebSocket RPC endpoint | `ws://127.0.0.1:9944` |
| `DATA_DIR` | Directory for storing data | `./data` |
| `RUST_LOG` | Log level | `storage_provider_node=debug` |

### Health check

```bash
curl http://localhost:3000/health
# {"status":"healthy","version":"0.1.0"}
```

### Running Both Nodes Together

For a storage provider, you need both nodes running:

```
Terminal 1 (Parachain):          Terminal 2 (Provider):
┌─────────────────────┐          ┌─────────────────────┐
│  polkadot-omni-node │◄────────►│ storage-provider-   │
│  --collator         │   RPC    │ node                │
│  --chain ...        │          │                     │
│  --runtime ...      │          │ Serves HTTP API     │
└─────────────────────┘          └─────────────────────┘
        │                                  │
        │ consensus                        │ data
        ▼                                  ▼
   Relay Chain                        Clients
```

## Client Usage

### Adding the client library

```toml
[dependencies]
storage-client = { path = "path/to/scalable-web3-storage/client" }
```

### Basic usage

```rust
use storage_client::{StorageClient, ChunkingStrategy};
use sp_core::H256;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to a provider node
    let client = StorageClient::new("http://localhost:3000");

    // Check provider health
    let health = client.health().await?;
    println!("Provider status: {}", health.status);

    // Upload data to a bucket
    let bucket_id = 1;
    let data = b"Hello, decentralized world!";

    let data_root = client
        .upload(bucket_id, data, ChunkingStrategy::default())
        .await?;

    println!("Data root: {:?}", data_root);

    // Commit the data to the bucket's MMR
    let commit = client.commit(bucket_id, vec![data_root]).await?;
    println!("MMR root: {}", commit.mmr_root);
    println!("Leaf index: {}", commit.leaf_indices[0]);

    // Read data back
    let read_data = client
        .read(&data_root, 0, data.len() as u64)
        .await?;

    assert_eq!(read_data, data);
    println!("Data verified!");

    Ok(())
}
```

### Upload large files

```rust
use storage_client::{StorageClient, ChunkingStrategy};
use std::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = StorageClient::new("http://localhost:3000");
    let bucket_id = 1;

    // Read a file
    let file_data = fs::read("my_file.pdf")?;

    // Upload with default 256 KiB chunks
    let data_root = client
        .upload(bucket_id, &file_data, ChunkingStrategy::default())
        .await?;

    // Or specify custom chunk size
    let data_root = client
        .upload(bucket_id, &file_data, ChunkingStrategy::Fixed(1024 * 1024)) // 1 MiB chunks
        .await?;

    // Commit
    let commit = client.commit(bucket_id, vec![data_root]).await?;

    println!("File stored at leaf index: {}", commit.leaf_indices[0]);

    Ok(())
}
```

### Check data existence

```rust
let hashes = vec![data_root1, data_root2, data_root3];
let result = client.check_exists(bucket_id, hashes).await?;

println!("Existing: {:?}", result.exists);
println!("Missing: {:?}", result.missing);
```

### Get bucket commitment

```rust
let commitment = client.get_commitment(bucket_id).await?;

println!("Bucket ID: {}", commitment.bucket_id);
println!("MMR root: {}", commitment.mmr_root);
println!("Start seq: {}", commitment.start_seq);
println!("Leaf count: {}", commitment.leaf_count);
```

## Provider Node API

### Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/info` | Provider information |
| `PUT` | `/node` | Upload a node (chunk or internal) |
| `GET` | `/node?hash=0x...` | Download a node |
| `POST` | `/exists` | Check which nodes exist |
| `POST` | `/commit` | Commit data roots to MMR |
| `GET` | `/read?data_root=...&offset=...&length=...` | Read chunks |
| `GET` | `/commitment?bucket_id=...` | Get current commitment |
| `GET` | `/mmr_proof?bucket_id=...&leaf_index=...` | Get MMR proof |
| `GET` | `/chunk_proof?data_root=...&chunk_index=...` | Get chunk proof |
| `GET` | `/buckets` | List all buckets |
| `POST` | `/delete` | Delete data (admin only) |
| `GET` | `/mmr_peaks?bucket_id=...` | Get MMR peaks (for replica sync) |
| `POST` | `/fetch_nodes` | Fetch multiple nodes (for replica sync) |

### Example: Upload and commit via curl

```bash
# 1. Upload a chunk
DATA=$(echo -n "Hello, World!" | base64)
HASH=$(echo -n "Hello, World!" | b2sum -l 256 | cut -d' ' -f1)

curl -X PUT http://localhost:3000/node \
  -H "Content-Type: application/json" \
  -d "{
    \"bucket_id\": 1,
    \"hash\": \"0x$HASH\",
    \"data\": \"$DATA\",
    \"children\": null
  }"

# 2. Commit to MMR
curl -X POST http://localhost:3000/commit \
  -H "Content-Type: application/json" \
  -d "{
    \"bucket_id\": 1,
    \"data_roots\": [\"0x$HASH\"]
  }"

# 3. Get commitment
curl "http://localhost:3000/commitment?bucket_id=1"
```

## Deployment

This section covers deploying to testnets and production. For local development, see [Quick Start](#quick-start).

### Deploy to Rococo Testnet

To deploy to the Rococo testnet:

1. **Build the runtime**:
```bash
cargo build --release -p storage-parachain-runtime
```

2. **Register your parachain**:
   - Go to [Rococo Faucet](https://faucet.polkadot.io/) to get test tokens
   - Reserve a ParaId on Rococo using the registrar pallet
   - Submit the runtime WASM and genesis state

3. **Run your collator**:
```bash
polkadot-omni-node \
  --collator \
  --chain chain-specs/storage-rococo.json \
  --runtime target/release/wbuild/storage-parachain-runtime/storage_parachain_runtime.compact.compressed.wasm \
  --relay-chain-rpc-urls wss://rococo-rpc.polkadot.io
```

4. **Run your provider node** (in a separate process):
```bash
PROVIDER_ID=<your-registered-provider-account> \
CHAIN_RPC=ws://127.0.0.1:9944 \
DATA_DIR=/var/lib/storage-provider \
cargo run --release -p storage-provider-node
```

### Chain Spec Configuration

The chain spec at `chain-specs/storage-rococo.json` contains:
- **Para ID**: 4000 (change this to your registered ID)
- **Token**: STOR with 12 decimals
- **Initial balances**: Pre-funded accounts for testing
- **Collators**: Initial collator set

To customize for your deployment:
1. Update the `para_id` to your registered parachain ID
2. Update initial balances and collators
3. Set the sudo key to your admin account

### Production Checklist

For production deployments, ensure:

- [ ] **Collator node**: Running with `--collator` flag, connected to relay chain
- [ ] **Provider node**: Running with correct `PROVIDER_ID` and `CHAIN_RPC`
- [ ] **Provider registered**: Account registered on-chain with sufficient stake
- [ ] **Firewall**: Collator P2P ports open (30333), provider HTTP port accessible
- [ ] **Monitoring**: Both nodes have health checks and logging configured
- [ ] **Backups**: Provider data directory backed up regularly

## On-Chain Integration

### Pallet configuration

```rust
impl pallet_storage_provider::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MaxMultiaddrLength = ConstU32<128>;
    type MaxMembers = ConstU32<100>;
    type MaxPrimaryProviders = ConstU32<5>;
    type MinProviderStake = ConstU64<1_000_000_000_000>; // 1 DOT
    type MaxChunkSize = ConstU32<262144>; // 256 KiB
    type ChallengeTimeout = ConstU64<14400>; // ~48 hours at 12s blocks
    type SettlementTimeout = ConstU64<7200>; // ~24 hours
    type RequestTimeout = ConstU64<1800>; // ~6 hours
}
```

### Key extrinsics

```rust
// Provider registration
StorageProvider::register_provider(origin, multiaddr, stake);
StorageProvider::add_stake(origin, amount);
StorageProvider::update_provider_settings(origin, settings);

// Bucket management
StorageProvider::create_bucket(origin, min_providers);
StorageProvider::set_member(origin, bucket_id, member, role);
StorageProvider::freeze_bucket(origin, bucket_id);

// Storage agreements
StorageProvider::request_primary_agreement(origin, bucket_id, provider, max_bytes, duration, max_payment);
StorageProvider::request_agreement(origin, bucket_id, provider, max_bytes, duration, max_payment, replica_params);
StorageProvider::accept_agreement(origin, bucket_id);
StorageProvider::end_agreement(origin, bucket_id, provider, action);

// Checkpoints
StorageProvider::checkpoint(origin, bucket_id, mmr_root, start_seq, leaf_count, signatures);

// Challenges
StorageProvider::challenge_checkpoint(origin, bucket_id, provider, leaf_index, chunk_index);
StorageProvider::respond_to_challenge(origin, challenge_id, response);
```

## Typical Workflow

1. **Provider setup** (on-chain)
   - Provider registers with stake
   - Provider configures settings (pricing, duration limits)

2. **Bucket creation** (on-chain)
   - Client creates bucket
   - Client adds members (writers, readers)
   - Client requests storage agreement with provider
   - Provider accepts agreement

3. **Data storage** (off-chain)
   - Client uploads chunks to provider
   - Client commits data roots to MMR
   - Provider signs commitment

4. **Checkpoint** (on-chain)
   - Client submits checkpoint with provider signatures
   - Providers become liable for committed data

5. **Verification** (off-chain)
   - Client spot-checks random chunks periodically
   - Client verifies data integrity via hashes

6. **Dispute** (on-chain, rare)
   - If provider fails to serve data, client challenges
   - Provider must respond with proof or be slashed

## License

Apache-2.0
