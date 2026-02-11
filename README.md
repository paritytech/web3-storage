# Scalable Web3 Storage

A decentralized storage system built on Substrate with game-theoretic guarantees. Storage providers lock stake and face slashing for data loss, while the chain acts as a credible threat rather than the hot path.

## What It Does

- **Storage providers** register with stake and offer storage services
- **Clients** create buckets and upload data off-chain
- **Storage agreements** bind providers to store data for agreed durations
- **Challenges** enforce accountability through slashing

Normal operations (reads, writes) happen off-chain. The chain is only touched for setup, checkpoints, and disputes.

## Quick Start

Get running in 5 minutes:

```bash
# Install just (command runner)
cargo install just

# One-time setup: downloads binaries + builds everything
just setup

# Start blockchain network + provider node
just start-chain     # Terminal 1
just start-provider  # Terminal 2

# Terminal 2:
# Setup (register provider, create bucket, establish agreement)
# Upload test data + challenge
just demo
```

**That's it!** Your local network is running with a provider ready to accept data.

### What Just Did

- Downloaded: `polkadot`, `polkadot-omni-node`, `zombienet`, `chain-spec-builder`
- Built: runtime, pallet, provider node, client SDK
- Started: Relay chain (2 validators) + Parachain (1 collator) + Provider node

### Next Steps

1. **Configure on-chain** - Register provider, create bucket, setup agreement
   - See: [Quick Start Guide](./docs/getting-started/QUICKSTART.md)

2. **Run tests** - Verify everything works
   ```bash
   bash scripts/verify-setup.sh  # Check on-chain setup
   bash scripts/quick-test.sh    # Run automated tests
   ```

3. **Try the demo** - Quick end-to-end test (after on-chain setup)
   ```bash
   just demo-setup   # Register provider, create bucket, establish agreement
   just demo-upload  # Upload test data with timestamp
   ```

4. **Upload data** - Use the client SDK or HTTP API
   - See: [Client Documentation](./client/README.md)

## Common Commands

```bash
just --list                  # Show all available commands
just check                   # Verify prerequisites
just build                   # Build the project
just start-chain             # Start blockchain only
just start-chain             # Start blockchain
just start-provider          # Start provider node
just health                  # Check provider health
```

## Documentation

📚 **[Full Documentation](./docs/README.md)** - Complete documentation index

### Quick Links

| Document | Description |
|----------|-------------|
| [Quick Start Guide](./docs/getting-started/QUICKSTART.md) | Get running fast (5 min) |
| [Manual Testing Guide](./docs/testing/MANUAL_TESTING_GUIDE.md) | Complete testing workflow |
| [Extrinsics Reference](./docs/reference/EXTRINSICS_REFERENCE.md) | Complete blockchain API |
| [Payment Calculator](./docs/reference/PAYMENT_CALCULATOR.md) | Calculate agreement costs |
| [Architecture Design](./docs/design/scalable-web3-storage.md) | System design & rationale |
| [Implementation Details](./docs/design/scalable-web3-storage-implementation.md) | Technical specs |

## Architecture

Two types of nodes work together:

```
┌──────────────────────────┐     ┌──────────────────────────┐
│   BLOCKCHAIN LAYER       │     │    STORAGE LAYER         │
│                          │     │                          │
│  Parachain Node          │────▶│  Provider Node           │
│  (Polkadot Omni Node)    │ RPC │  (HTTP Server)           │
│                          │     │                          │
│  • Stake & registration  │     │  • Data storage          │
│  • Agreements            │     │  • MMR commitments       │
│  • Checkpoints           │     │  • Chunk serving         │
│  • Challenges/slashing   │     │  • Replica sync          │
└──────────────────────────┘     └──────────────────────────┘
      Infrequent                        Hot path
   (setup, disputes)               (all data operations)
```

### Two Nodes, Two Purposes

| Node | Purpose | Run by |
|------|---------|--------|
| **Parachain Node** (Omni Node + Runtime) | Blockchain consensus, state transitions, finality | Collators (parachain validators) |
| **Provider Node** (HTTP Server) | Store actual data, serve clients, respond to challenges | Storage providers |

**Storage providers run both nodes:**
- Parachain node: Participates in blockchain consensus
- Provider node: Handles actual data storage/serving

## Project Structure

```
scalable-web3-storage/
├── pallet/               # Substrate pallet (on-chain logic)
├── runtime/              # Parachain runtime
├── provider-node/        # Off-chain storage server (HTTP API)
├── client/               # Client SDK for applications
├── primitives/           # Shared types and utilities
├── scripts/              # Helper scripts
└── docs/                 # Documentation
    ├── getting-started/  # Quick start guides
    ├── testing/          # Testing procedures
    ├── reference/        # API references
    └── design/           # Architecture docs
```

## Development

### Prerequisites

- Rust 1.74+ with wasm32-unknown-unknown target
- Cargo

### Build

```bash
# Build everything
cargo build --release

# Or use just
just build
```

### Testing

```bash
# Unit tests
cargo test

# Integration tests with running system
just start-chain            # Terminal 1
just start-provider         # Terminal 2
just demo  # Terminal 3
```

### Provider Node Configuration

The provider node uses environment variables for configuration:

| Variable | Description | Default |
|----------|-------------|---------|
| `PROVIDER_ID` | Provider's on-chain account ID (SS58 format) | **Required** |
| `CHAIN_RPC` | Parachain WebSocket RPC endpoint | `ws://127.0.0.1:9944` |
| `BIND_ADDR` | HTTP server bind address | `0.0.0.0:3000` |
| `DATA_DIR` | Directory for storing data | `./data` |
| `RUST_LOG` | Log level configuration | `storage_provider_node=debug` |

## Example: Basic Upload Flow

```rust
use storage_client::StorageUserClient;

// Connect to provider
let mut client = StorageUserClient::new(config);
client.connect_chain().await?;

// Upload data (off-chain)
let data = b"Hello, decentralized storage!";
let result = client.upload(bucket_id, data).await?;

// Verify upload
let downloaded = client.download(bucket_id, result.seq).await?;
assert_eq!(data, downloaded);
```

See [Client README](./client/README.md) for complete examples.

## Key Features

- **Off-chain storage**: All data operations happen off-chain via HTTP
- **On-chain accountability**: Stake-based provider registration with slashing
- **Content-addressed**: All data is blake2-256 content-addressed
- **MMR commitments**: Merkle Mountain Range for efficient proofs
- **Challenge mechanism**: Anyone can challenge providers to prove data possession
- **Replica support**: Primary providers can sync to replica providers
- **Flexible agreements**: Customizable duration, capacity, pricing per provider

## Workflow

1. **Provider Setup (on-chain)**
   - Provider registers with stake
   - Provider configures settings (pricing, duration limits)

2. **Bucket Creation (on-chain)**
   - Client creates bucket
   - Client adds members (writers, readers)
   - Client requests storage agreement with provider
   - Provider accepts agreement

3. **Data Storage (off-chain)**
   - Client uploads chunks to provider via HTTP
   - Provider stores and builds MMR commitment
   - Provider signs commitment

4. **Checkpoint (on-chain)**
   - Client submits checkpoint with provider signatures
   - Providers become liable for committed data

5. **Verification (off-chain)**
   - Client spot-checks random chunks periodically
   - Client verifies data integrity via hashes

6. **Dispute (on-chain, rare)**
   - If provider fails to serve data, client challenges
   - Provider must respond with proof or be slashed

## Deployment

See [Manual Testing Guide](./docs/testing/MANUAL_TESTING_GUIDE.md) for:
- Local development setup
- Rococo testnet deployment
- Production deployment checklist

## Contributing

1. Read [CLAUDE.md](./CLAUDE.md) - Project overview, build commands, and code review guidelines
2. Read the [Architecture Design](./docs/design/scalable-web3-storage.md)
3. Check [Implementation Details](./docs/design/scalable-web3-storage-implementation.md)
4. Run tests: `cargo test`
5. Follow existing code style: `cargo fmt --check`

## License

Apache-2.0
