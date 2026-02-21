# CLAUDE.md - Scalable Web3 Storage

## Claude Preferences

**Git commit rules:**
- NEVER add Co-Authored-By lines to commits
- NEVER use git rebase
- NEVER use git push --force or git push -f

## Project Overview

Scalable Web3 Storage is a decentralized storage system built on Substrate with game-theoretic guarantees. Storage providers lock stake and face slashing for data loss, while the chain acts as a credible threat rather than the hot path.

**Architecture**: Two-node system where blockchain handles accountability and provider nodes handle actual storage:
- **Parachain Node**: On-chain logic for stake, agreements, checkpoints, and challenges
- **Provider Node**: Off-chain HTTP server for data upload, download, and MMR commitment

**Key Purpose**: Enable trustless storage where normal operations (reads, writes) happen off-chain via HTTP, and the chain is only touched for setup, checkpoints, and disputes.

## Build Commands

```bash
# Build everything (release)
cargo build --release

# Build specific components
cargo build --release -p storage-parachain-runtime
cargo build --release -p storage-provider-pallet
cargo build --release -p storage-provider-node
cargo build --release -p storage-client

# Build with runtime benchmarks
cargo build --release --features runtime-benchmarks

# Using just (recommended)
just build
```

## Test Commands

```bash
# Run all tests
cargo test

# Run pallet tests
cargo test -p storage-provider-pallet

# Run provider node tests
cargo test -p storage-provider-node

# Run client SDK tests
cargo test -p storage-client

# Run integration tests (requires running services)
just start-chain     # Terminal 1
just start-provider  # Terminal 2
just demo  # Terminal 3

# Clippy linting
cargo clippy --all-targets --all-features --workspace -- -D warnings
```

## Formatting

```bash
# Rust formatting (requires nightly)
cargo +nightly fmt --all

# TOML formatting
taplo format --check --config .config/taplo.toml

# Feature propagation lint (checks Cargo.toml feature gates)
zepter run --config .config/zepter.yaml
```

## Run Commands

```bash
# One-time setup (downloads binaries, builds project)
just setup

# Start blockchain
just start-chain

# Start provider node manually
just start-provider

# Check provider health
just health

# Verify on-chain setup
bash scripts/verify-setup.sh

# Run automated tests
just demo
```

## Architecture

### Directory Structure

```
web3-storage/
├── pallet/                     # Substrate pallet (on-chain logic)
│   ├── src/lib.rs             # Core pallet implementation
│   └── Cargo.toml             # Pallet dependencies
├── runtime/                    # Parachain runtime
│   ├── src/lib.rs             # Runtime configuration
│   └── Cargo.toml             # Runtime dependencies
├── provider-node/              # Off-chain HTTP storage server
│   ├── src/                   # Provider implementation
│   │   ├── main.rs           # Server entry point
│   │   ├── storage.rs        # Storage layer
│   │   └── mmr.rs            # MMR commitment logic
│   └── Cargo.toml            # Provider dependencies
├── client/                     # Client SDK for applications
│   ├── src/                   # SDK implementation
│   │   ├── lib.rs            # Main client API
│   │   └── types.rs          # Client types
│   ├── examples/             # Usage examples
│   └── README.md             # SDK documentation
├── primitives/                 # Shared types and utilities
│   ├── src/lib.rs            # Common types
│   └── Cargo.toml            # Primitive dependencies
├── scripts/                    # Helper scripts
│   ├── quick-test.sh         # Automated basic tests
│   ├── verify-setup.sh       # On-chain setup verification
│   └── check-chain.sh        # Blockchain health check
├── chain-specs/                # Chain specification files
├── docs/                       # Documentation
│   ├── README.md             # Documentation index
│   ├── getting-started/      # Quick start guides
│   ├── testing/              # Testing procedures
│   ├── reference/            # API references
│   └── design/               # Architecture docs
└── justfile                    # Development commands
```

### Key Components

**Pallet (`pallet/`)**: On-chain logic for provider registration, bucket creation, storage agreements, checkpoints, and challenge/slashing mechanism.

**Runtime (`runtime/`)**: Parachain runtime that includes the storage provider pallet and configures its parameters (stake requirements, challenge periods, etc.).

**Provider Node (`provider-node/`)**: Off-chain HTTP server that:
- Stores data chunks locally
- Builds MMR commitments
- Serves data via HTTP API
- Signs checkpoints for on-chain submission

**Client SDK (`client/`)**: Rust library for applications to:
- Create buckets and agreements (on-chain)
- Upload/download data (off-chain HTTP)
- Submit checkpoints (on-chain)
- Challenge providers (on-chain)

**Primitives (`primitives/`)**: Shared types used across pallet, provider node, and client.

## Development Workflow

### Quick Start
1. **Setup**: `just setup` (one-time, downloads binaries and builds)
2. **Start**: `just start-chain` then `just start-provider` (in separate terminals)
3. **Configure**: Follow [Quick Start Guide](docs/getting-started/QUICKSTART.md) for on-chain setup
4. **Test**: `just demo`

### Development Cycle
1. **Format code**: `cargo fmt --all`
2. **Run clippy**: `cargo clippy --all-targets --all-features --workspace`
3. **Run tests**: `cargo test`
4. **Build**: `cargo build --release` or `just build`

### Local Testing with Zombienet

The project uses Zombienet for local relay chain + parachain testing:

```bash
# Start network (relay chain + parachain)
just start-chain

# Or manually:
.bin/zombienet spawn zombienet.toml
```

**Network URLs**:
- Relay chain: `ws://127.0.0.1:9900`
- Parachain: `ws://127.0.0.1:2222`
- Provider HTTP: `http://localhost:3333`

**Web UI**:
- Relay chain: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900
- Parachain: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222

## Polkadot SDK (Upstream)

This project is built on the **Polkadot SDK** (formerly Substrate). For deeper understanding of FRAME pallets, runtime macros, and consensus:

- **Repository**: https://github.com/paritytech/polkadot-sdk
- **Documentation**: https://paritytech.github.io/polkadot-sdk/

The Polkadot SDK provides:
- FRAME pallet system and runtime macros
- Parachain consensus (Cumulus)
- Networking (libp2p)
- RPC infrastructure
- XCM (Cross-Consensus Messaging)

## Dependencies

- **Polkadot SDK**: See `Cargo.toml` workspace dependencies
- **Rust**: 1.74+ with `wasm32-unknown-unknown` target
- **Just**: Command runner (`cargo install just`)
- **Zombienet**: Network spawner (auto-downloaded by `just setup`)
- **Polkadot**: Relay chain binary (auto-downloaded)
- **Polkadot Omni Node**: Parachain node (auto-downloaded)

## Configuration

### Runtime Parameters (runtime/src/lib.rs)

```rust
// Token decimals
pub const UNIT: Balance = 1_000_000_000_000; // 12 decimals

// Minimum provider stake: 1000 tokens
pub const MinProviderStake: Balance = 1_000 * UNIT;

// Challenge period: 100 blocks
pub const ChallengePeriod: BlockNumber = 100;

// Checkpoint threshold: 51% of providers must sign
pub const CheckpointThreshold: Permill = Permill::from_percent(51);
```

### Provider Settings (configured per provider)

```rust
pub struct ProviderSettings {
    min_duration: BlockNumber,        // Minimum agreement duration
    max_duration: BlockNumber,        // Maximum agreement duration
    price_per_byte: Balance,          // Price per byte per block
    accepting_primary: bool,          // Accepting new agreements
    replica_sync_price: Option<Balance>, // Price for replica sync
    accepting_extensions: bool,       // Accepting agreement extensions
}
```

## Key Concepts

### Storage Flow

1. **Setup (on-chain)**:
   - Provider registers with stake
   - Client creates bucket
   - Agreement established

2. **Storage (off-chain)**:
   - Client uploads chunks via HTTP to provider
   - Provider stores and builds MMR commitment

3. **Checkpoint (on-chain)**:
   - Provider signs MMR root
   - Client submits checkpoint
   - Provider now liable for data

4. **Verification (off-chain)**:
   - Client spot-checks chunks
   - Client can download anytime

5. **Dispute (on-chain, rare)**:
   - Client submits challenge
   - Provider must provide proof or get slashed

### MMR (Merkle Mountain Range)

The provider builds an MMR over stored chunks:
- Each upload adds a leaf to the MMR
- MMR root represents commitment to all data
- Efficient proofs for individual chunks
- Enables challenge mechanism

### Payment Calculation

```
payment = price_per_byte × max_bytes × duration
```

Example:
```
price_per_byte = 1,000,000
max_bytes = 1,073,741,824 (1 GB)
duration = 500 blocks
payment = 536,870,912,000,000,000
```

Set `maxPayment` with 10-20% buffer to account for price changes.

## Code Review Guidelines (Parity Standards)

These guidelines are used by the Claude Code review bot and should be followed by all contributors.

### Rust Code Quality

- **Error Handling**: Use `Result` types with meaningful error enums. Avoid `unwrap()` and `expect()` in production code; they are acceptable in tests.
- **Arithmetic Safety**: Use `checked_*`, `saturating_*`, or `wrapping_*` arithmetic to prevent overflow. Never use raw arithmetic operators on user-provided values.
- **Naming**: Follow Rust naming conventions (snake_case for functions/variables, CamelCase for types).
- **Complexity**: Prefer simple, readable code. Avoid over-engineering and premature abstractions.
- **No useless comments**: Comments should mostly explain **why** things are done, not **how**. The code should be readable enough to explain the how.

### FRAME Pallet Standards

- **Storage**: Use appropriate storage types (`StorageValue`, `StorageMap`, `StorageDoubleMap`, `CountedStorageMap`).
- **Events**: Emit events for all state changes that external observers need to track.
- **Errors**: Define descriptive error types in the pallet's `Error` enum.
- **Weights**: All extrinsics must have accurate weight annotations. Update benchmarks when logic changes.
- **Origins**: Use the principle of least privilege for origin checks.
- **Hooks**: Be cautious with `on_initialize` and `on_finalize`; they affect block production time. Never panic or do unbounded iteration in them. Always benchmark them properly.

### Security Considerations

- **No Panics in Runtime**: Runtime code must never panic. Use defensive programming with `defensive_*` macros.
- **Bounded Collections**: Use `BoundedVec`, `BoundedBTreeMap` etc. to prevent unbounded storage growth.
- **Input Validation**: Validate all user inputs at the entry point.
- **Storage Deposits**: Consider requiring deposits for user-created storage items.
- **Arithmetic**: Always use checked arithmetic for financial calculations.
- **Access Control**: Verify origin permissions before state changes.

### Testing Requirements

- **Unit Tests**: All new functionality requires unit tests.
- **Edge Cases**: Test boundary conditions, error paths, and malicious inputs.
- **Integration Tests**: Complex features should have integration tests.
- **Mock Tests**: Use `mock.rs` and `TestExternalities` for pallet tests.
- **Provider Node Tests**: Test HTTP API endpoints and storage layer.
- **Client SDK Tests**: Test all public SDK methods.

### PR Requirements

- **Single Responsibility**: Each PR should address one concern.
- **Tests Pass**: All CI checks must pass (`cargo test`, `cargo clippy`, `cargo fmt`).
- **No Warnings**: Code should compile without warnings.
- **Documentation**: Public APIs require rustdoc comments.
- **Changelog**: Update changelog for user-facing changes.

## Documentation

📚 **[Complete Documentation](docs/README.md)** - Full documentation index

### Quick Links

| Document | Description |
|----------|-------------|
| [Quick Start Guide](docs/getting-started/QUICKSTART.md) | Get running in 5 minutes |
| [Manual Testing Guide](docs/testing/MANUAL_TESTING_GUIDE.md) | Complete testing workflow |
| [Extrinsics Reference](docs/reference/EXTRINSICS_REFERENCE.md) | Complete blockchain API |
| [Payment Calculator](docs/reference/PAYMENT_CALCULATOR.md) | Calculate agreement costs |
| [Benchmarking Guide](docs/reference/BENCHMARKING.md) | Generate weights for extrinsics |
| [Architecture Design](docs/design/scalable-web3-storage.md) | System design & rationale |
| [Implementation Details](docs/design/scalable-web3-storage-implementation.md) | Technical specs |

## Common Issues & Solutions

### "Insufficient Stake" Error
- Minimum required: 1000 tokens = `1000000000000000` (12 decimals)
- Check Alice's balance in Accounts tab

### "PaymentExceedsMax" Error
- Calculate payment: `price_per_byte × max_bytes × duration`
- Set `maxPayment` with 10-20% buffer
- See [Payment Calculator](docs/reference/PAYMENT_CALCULATOR.md)

### Upload Fails
- Complete on-chain setup first: register provider, create bucket, establish agreement
- Run `bash scripts/verify-setup.sh` to check prerequisites

### Provider Not Accepting Agreements
- Call `updateProviderSettings` after registration
- Set `acceptingPrimary: true`

## Feature Flags

- `runtime-benchmarks` - Enable weight generation
- `try-runtime` - Runtime migration testing
- `std` - Standard library features (default)

## Notes

- Token decimals: 12 (like Polkadot)
- Minimum stake: 1000 tokens
- Challenge period: 100 blocks
- Data is content-addressed with blake2-256
- All data operations happen off-chain via HTTP
- Chain is only for accountability and disputes
