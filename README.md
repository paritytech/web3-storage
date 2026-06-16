# Scalable Web3 Storage

> [!WARNING]                                                                                                                            
> This is a reference implementation provided for research, experimentation, and developer education. This code has not been audited. It is actively under development and may contain bugs, vulnerabilities, or incomplete features. It is not recommended for production use without independent review. Use at your own risk.

A decentralized storage system built on Substrate with game-theoretic guarantees. Storage providers lock stake and face slashing for data loss, while the chain acts as a credible threat rather than the hot path.

[![License: GPL-3.0-only](https://img.shields.io/badge/License-GPL--3.0--only-blue.svg)](LICENSE-GPL3)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE-APACHE2)
[![Security: unaudited](https://img.shields.io/badge/security-unaudited-red.svg)](https://github.com/paritytech/polkadot-bulletin-chain/security) 
[![Status: prototype](https://img.shields.io/badge/status-experimental-yellow.svg)](#)
[![Substrate](https://img.shields.io/badge/built%20with-Polkadot%20SDK-green.svg)](#)

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

1. **`just demo`** drives the full Layer-0 happy path against an already-running
   chain + provider: registers a provider, creates a bucket, opens an agreement,
   uploads data, fires two challenges, and asserts the provider defends both.
   This is also what CI runs. It does **not** start the chain or provider for
   you — keep `just start-chain` and `just start-provider` running in two other
   terminals first.

2. **Inspect chain health**: `bash scripts/check-chain.sh` (relay + parachain
   status), `just health` (provider node health), `just stats` (provider stats).

3. **Build something on top**: see [Client Documentation](./client/README.md)
   for the Layer-0 SDK, or [`FILE_SYSTEM_QUICKSTART.md`](./FILE_SYSTEM_QUICKSTART.md)
   for the Layer-1 file-system interface.

## File System Interface (Layer 1)

The Layer 1 File System Interface provides a familiar file/folder abstraction
over Layer 0's raw blob storage.

### Quick Start with File System

```bash
# In separate terminals:
just start-chain            # Terminal 1: relay + parachain
just start-provider         # Terminal 2: provider node

# Then run the file-system integration example:
just fs-demo-ci             # Terminal 3
```

**What you get:**
- ✅ Familiar file/folder interface
- ✅ Automatic provider selection
- ✅ Built-in blockchain integration

### File System Commands

```bash
just fs-test-all            # Run all unit tests across primitives, pallet, client
just fs-demo-ci             # Integration example against a running chain + provider
```

**Complete guide**: [FILE_SYSTEM_QUICKSTART.md](./FILE_SYSTEM_QUICKSTART.md)

### When to Use Layer 0 vs Layer 1

**Use Layer 1 (File System)** if you:
- Want a familiar file/folder interface
- Need automatic setup and provider selection
- Are building a general-purpose file storage app
- Prefer simplicity over low-level control

**Use Layer 0 (Direct Storage)** if you:
- Need full control over storage operations
- Are building custom storage logic
- Want to implement your own data structures
- Need direct access to buckets and agreements

## Common Commands

```bash
# General
just --list                  # Show all available commands
just build                   # Build the project
just setup                   # One-time: download binaries + build everything

# Infrastructure
just start-chain             # Start relay + parachain
just start-provider          # Start provider node
just health                  # Check provider health
just stats                   # Provider storage stats

# End-to-end demos (require chain + provider running)
just demo                    # Layer-0 PAPI demo: setup, upload, 2 challenges
just fs-demo-ci              # Layer-1 file-system integration example
just s3-demo-ci              # Layer-1 S3-compatible integration example

# Tests
cargo test --workspace       # All unit + integration tests
just fs-test-all             # File-system layer only
just s3-test-all             # S3 layer only
```

## Documentation

📚 **[Full Documentation](./docs/README.md)** - Complete documentation index

### Quick Links

| Document | Description |
|----------|-------------|
| **[Layer 1 Quick Start](./docs/getting-started/LAYER1_QUICKSTART.md)** | **Three-terminal setup + SDK examples (recommended)** |
| [File System Quick Start](./FILE_SYSTEM_QUICKSTART.md) | File-system-only quickstart |
| [File System Docs](./docs/filesystems/README.md) | Complete Layer 1 documentation |
| [Extrinsics Reference](./docs/reference/EXTRINSICS_REFERENCE.md) | Complete blockchain API |
| [Payment Calculator](./docs/reference/PAYMENT_CALCULATOR.md) | Calculate agreement costs |
| [Architecture Design](./docs/design/scalable-web3-storage.md) | System design, economics, common concerns |
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

- Rust toolchain: 
   ```toml
   channel = "1.93.0"
   targets = ["wasm32v1-none"]
   components = ["clippy", "rust-src", "rustfmt"]
   ```
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
| `CHAIN_RPC` | Parachain WebSocket RPC endpoint | `ws://127.0.0.1:2222` |
| `BIND_ADDR` | HTTP server bind address | `0.0.0.0:3333` |
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

For local dev, follow [Layer 1 Quick Start](./docs/getting-started/LAYER1_QUICKSTART.md). For testnet/production, no canonical guide exists yet — see `chain-specs/` and `zombienet.toml` for current local network shape.

## Contributing

1. Read [CLAUDE.md](./CLAUDE.md) - Project overview, build commands, and code review guidelines
2. Read the [Architecture Design](./docs/design/scalable-web3-storage.md)
3. Check [Implementation Details](./docs/design/scalable-web3-storage-implementation.md)
4. Run tests: `cargo test`
5. Follow existing code style: `cargo fmt --check`

## Security

The security policy for this project is governed by the
[paritytech organization-level security policy](https://github.com/paritytech/.github/blob/master/SECURITY.md).
If you discover a vulnerability, please follow the responsible disclosure process described there.

## License

This project is dual-licensed:

- **Runtime, provider node, and user-interface applications** are licensed under
  [GPL-3.0-only](LICENSE-GPL3).
- **Pallets, primitives, client SDKs, and shared libraries** are licensed under
  [Apache-2.0](LICENSE-APACHE2).

Each crate and package declares its applicable license in its `Cargo.toml` or
`package.json`. See the individual LICENSE files at the repository root for the
full license texts.
