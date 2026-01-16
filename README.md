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

```
┌─────────────────────────────────────────────────────────────────────┐
│                           ON-CHAIN                                  │
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                    pallet-storage-provider                    │  │
│  │  ├── Providers: registration, stake, settings                 │  │
│  │  ├── Buckets: membership, snapshots, agreements               │  │
│  │  ├── StorageAgreements: primary & replica contracts           │  │
│  │  └── Challenges: dispute resolution, slashing                 │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│    Chain touched for: bucket creation, agreement setup,             │
│    checkpoints (infrequent), disputes (rare)                        │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
                                    ▲
                                    │ rare interactions
                                    │
┌─────────────────────────────────────────────────────────────────────┐
│                          OFF-CHAIN                                  │
│                                                                     │
│   ┌─────────────┐    writes     ┌─────────────────────────────┐    │
│   │   Client    │ ────────────► │    Provider Node            │    │
│   │  (storage-  │               │  (storage-provider-node)    │    │
│   │   client)   │ ◄──────────── │                             │    │
│   └─────────────┘    reads      │  • HTTP API                 │    │
│                                 │  • Content-addressed store  │    │
│                                 │  • MMR commitments          │    │
│                                 └─────────────────────────────┘    │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

## Project Structure

```
scalable-web3-storage/
├── primitives/           # Shared types (BucketId, Role, MMR types, etc.)
├── pallet/               # Substrate pallet (on-chain logic)
├── provider-node/        # Off-chain provider node (HTTP server)
├── client/               # Client library for storage operations
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

### Start a provider node

```bash
# Default configuration (port 3000)
cargo run -p storage-provider-node

# Custom port and provider ID
BIND_ADDR=0.0.0.0:8080 PROVIDER_ID=0xYourProviderAddress cargo run -p storage-provider-node
```

### Environment variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BIND_ADDR` | Address to bind the HTTP server | `0.0.0.0:3000` |
| `PROVIDER_ID` | Provider's on-chain account ID | `0x0000...` |
| `RUST_LOG` | Log level | `storage_provider_node=debug` |

### Health check

```bash
curl http://localhost:3000/health
# {"status":"healthy","version":"0.1.0"}
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
