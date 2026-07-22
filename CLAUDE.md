# CLAUDE.md - Scalable Web3 Storage

## Agent Rules

**Git commit rules:**
- NEVER add Co-Authored-By lines to commits
- NEVER use git rebase
- NEVER use git push --force or git push -f

**Pull request rules:**
- ALWAYS open pull requests against the repository's default branch (`dev`)

**Code review rules:**
- NEVER submit AI-generated review comments (PR reviews, inline comments, or issue comments) to GitHub automatically
- ALWAYS present review findings to the human reviewer for triage first, and only post the ones they explicitly approve, after they explicitly ask for them to be posted

**Cargo dependency rules:**
- ALWAYS declare external dependencies in the root `[workspace.dependencies]` and inherit them in crates via `{ workspace = true }`. Never add inline-versioned dependencies (e.g. `foo = "1.2"`) to a crate's `Cargo.toml`.
- On the inheriting line you may only add `features` (additive) and `optional`; per Cargo, `version` and `default-features` cannot appear there, so set `default-features` in the workspace declaration (e.g. `hex = { version = "0.4", default-features = false }`).

**Automatic formatting:**
- ALWAYS run `/format` after generating or modifying Rust code
- ALWAYS run `/format` before creating any git commit
- This ensures all code follows project formatting standards (Rust, TOML, feature propagation) and passes clippy

**Design & spec discipline:**
- `docs/design/` is the **canonical, review-gated source of truth** (enforced by `.github/CODEOWNERS`). Reason and implement *from* it; treat it as the spec.
- **Validate code against the design.** When writing or changing code, check it against `docs/design/`. On any divergence, **stop and flag** — don't proceed on assumptions.
- **Prefer flagging over quietly editing the design to match the code.** If implementation and design disagree, treat it as a *finding*: open or reference an issue and discuss before changing the spec, rather than silently reconciling the gap.
- If something in the design looks **wrong or vulnerable**, **flag and discuss** (open an issue and ping the design owner) — don't just fix it. Changes to the design itself go through a PR reviewed per `.github/CODEOWNERS`.
- **`docs/reference/`** is *derived* documentation and **not** gated. When you change behavior, **update the relevant `reference/` doc** so it keeps reflecting the implementation.
- **`docs/drafts/`** is unratified / WIP — don't treat it as authoritative or reason from it as if it were the spec.

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
cargo build --release -p pallet-storage-provider
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
cargo test -p pallet-storage-provider

# Run provider node tests
cargo test -p storage-provider-node

# Run client SDK tests
cargo test -p storage-client

# Run file system tests (Layer 1)
cargo test -p file-system-primitives
cargo test -p pallet-drive-registry
cargo test -p file-system-client

# Or test all file system components at once
just fs-test-all

# Run integration tests (require chain + provider already running)
just start-chain     # Terminal 1
just start-provider  # Terminal 2
just demo            # Terminal 3 — Layer-0 PAPI flow
just fs-demo-ci      # Terminal 3 — Layer-1 file-system flow
just s3-demo-ci      # Terminal 3 — Layer-1 S3 flow

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

# Check chain health (relay + parachain + current block)
bash scripts/check-chain.sh

# Run end-to-end PAPI demo (setup, upload, 2 challenges)
just demo
```

### Running the UIs locally

When the user says **"run locally"** (or "run the UIs", "start the UIs", "spin up the UIs"), invoke the `run-local-uis` project skill — it starts all four `user-interfaces/` apps on their canonical ports with Vite HMR, including the landing page (which needs a custom dev config to substitute its build-time placeholders and rewrite card links). Canonical ports: landing 5176, drive-ui 5174, provider 5175, s3-ui 5177.

## File System (Layer 1) Commands

The File System Interface provides a high-level abstraction over Layer 0's raw blob storage.

```bash
# Test all file system components (primitives + pallet + client)
just fs-test-all

# Run integration example against a running chain + provider node
just fs-demo-ci

# Manually run the basic_usage example
cargo run -p file-system-client --example basic_usage
```

**Quick Start Guide**: [FILE_SYSTEM_QUICKSTART.md](docs/getting-started/FILE_SYSTEM_QUICKSTART.md)

**Complete Documentation**: [docs/filesystems/README.md](./docs/filesystems/README.md)

## JS/TS: use `polkadot-api`, never `@polkadot/*`

For any JavaScript or TypeScript code in this repo (demos, scripts, tooling, future SDKs), talk to the chain through `polkadot-api` (PAPI).
Do NOT introduce `@polkadot/keyring`, `@polkadot/util-crypto`, `@polkadot/util`, `@polkadot/api`, or any other `@polkadot/*` package.
They duplicate functionality PAPI already provides, drag in 20+ transitive deps, and force `cryptoWaitReady()` awaits everywhere. Use these instead:

| Need                                 | Use                                                                                                |
| ------------------------------------ | -------------------------------------------------------------------------------------------------- |
| Chain client + typed API             | `polkadot-api` (`createClient`; `getWsProvider` from `polkadot-api/ws`)                   |
| Signer wrapper                       | `getPolkadotSigner` from `polkadot-api/signer`                                                     |
| SCALE / `Binary` / `Enum`            | `import { Binary, Enum } from "polkadot-api"` — NOT `@polkadot-api/substrate-bindings` (its 0.20+ `Binary` is a codec helper without `fromBytes`/`asBytes`) |
| Sr25519 key derivation (`//Alice`)   | `sr25519CreateDerive` from `@polkadot-labs/hdkd` + `DEV_PHRASE` + `entropyToMiniSecret` + `mnemonicToEntropy` from `@polkadot-labs/hdkd-helpers` |
| SS58 encode / decode                 | `ss58Address` / `ss58Decode` from `@polkadot-labs/hdkd-helpers`                                    |
| blake2-256 hashing                   | `blake2b256` from `@polkadot-labs/hdkd-helpers`                                                    |
| `cryptoWaitReady()`                  | Not needed — hdkd is synchronous; delete the import and the await                                  |

In-repo code should not hand-roll these: the workspace package `@web3-storage/sdk` (`packages/sdk`) already provides `connect`, `makeSigner`, `seedToKeypair`, the `Alice..Ferdie` dev signers, `submitTx` (in-block by default, per the suite's finalization semantics), `watchValue`-based waits, and typed wrappers for every pallet extrinsic. Import from it instead of duplicating the patterns below.

Canonical signer/derive pattern (what `makeSigner` does under the hood) — set up the derive function once at module load, then call `makeSigner("//Alice")` etc.:

```js
import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
  ss58Address,
  ss58Decode,
} from "@polkadot-labs/hdkd-helpers";

const devMiniSecret = entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE));
const deriveSr25519 = sr25519CreateDerive(devMiniSecret);

export function makeSigner(seed) {
  const keyPair = deriveSr25519(seed); // seed is a SURI path like "//Alice"
  return {
    signer: getPolkadotSigner(keyPair.publicKey, "Sr25519", keyPair.sign),
    address: ss58Address(keyPair.publicKey), // prefix 42 (`5…`), same as @polkadot/keyring default
    publicKey: keyPair.publicKey,
    seed,
  };
}
```

`ss58Address` defaults to substrate prefix 42 (`5…`) while PAPI surfaces accounts with the runtime SS58 prefix (Polkadot-style `1…` on this parachain) — same key, different string, so string equality fails. Compare raw bytes via `ss58Decode`:

```js
// ss58Decode(addr) → [bytes, prefix]
export function sameAddress(a, b) {
  try {
    const [aBytes] = ss58Decode(a);
    const [bBytes] = ss58Decode(b);
    if (aBytes.length !== bBytes.length) return false;
    for (let i = 0; i < aBytes.length; i++) {
      if (aBytes[i] !== bBytes[i]) return false;
    }
    return true;
  } catch {
    return false;
  }
}
```

## Architecture

### Directory Structure

```
web3-storage/
├── crates/
│   └── pallets/               # FRAME pallets (on-chain logic)
│       ├── storage-provider/  # Storage provider pallet (Layer 0)
│       ├── drive-registry/    # Drive Registry pallet (Layer 1 file system)
│       └── s3-registry/       # S3 Registry pallet (Layer 1 S3)
├── runtimes/                   # Parachain runtimes
│   ├── web3-storage-local/     # Local testnet runtime (storage-parachain-runtime)
│   │   ├── src/lib.rs         # Runtime configuration
│   │   └── Cargo.toml         # Runtime dependencies
│   └── web3-storage-paseo/     # Paseo testnet runtime (storage-paseo-runtime)
├── provider-node/              # Off-chain HTTP storage server
│   ├── src/                   # Provider implementation
│   │   ├── main.rs           # Server entry point
│   │   ├── storage.rs        # Storage layer
│   │   └── mmr.rs            # MMR commitment logic
│   └── Cargo.toml            # Provider dependencies
├── client/                     # Layer 0 Client SDK
│   ├── src/                   # SDK implementation
│   │   ├── lib.rs            # Main client API
│   │   └── types.rs          # Client types
│   ├── examples/             # Usage examples
│   └── README.md             # SDK documentation
├── primitives/                 # Layer 0 shared types and utilities
│   ├── src/lib.rs            # Common types
│   └── Cargo.toml            # Primitive dependencies
├── precompiles/                # pallet_revive precompiles (Solidity → pallets)
│   ├── storage-provider-precompile/  # 0x…0901 — bucket lifecycle
│   └── drive-registry-precompile/    # 0x…0902 — drive lifecycle
├── examples/contracts/         # Example Solidity dApps (StorageMarketplace.sol)
├── storage-interfaces/         # Layer 1 - High-level interfaces
│   └── file-system/           # File System Interface
│       ├── primitives/        # File system types (DriveInfo, CommitStrategy, etc.)
│       └── client/            # File System Client SDK
│           ├── src/
│           │   ├── lib.rs     # Main file system client
│           │   └── substrate.rs # Blockchain integration (subxt)
│           ├── examples/
│           │   └── basic_usage.rs # Complete workflow example
│           └── README.md      # File system client docs
├── scripts/                    # Helper scripts
│   ├── build-chain-spec.sh   # Build runtime + emit chain spec (used by `just generate-chain-spec`)
│   ├── check-chain.sh        # Relay + parachain health probe
│   └── quick-test.sh         # Curl-based smoke test of provider HTTP API
├── chain-specs/                # Chain specification files
├── docs/                       # Documentation
│   ├── README.md             # Documentation index
│   ├── getting-started/      # Quick start guides
│   ├── testing/              # Testing procedures
│   ├── reference/            # API references
│   ├── design/               # Architecture docs
│   └── filesystems/          # Layer 1 File System docs
│       ├── README.md         # File system overview
│       ├── ARCHITECTURE.md   # Encoding, security, chain integration
│       ├── USER_GUIDE.md     # User guide
│       ├── API_REFERENCE.md  # API documentation
│       └── ADMIN_GUIDE.md    # Admin guide
└── justfile                    # Development commands
```

### Key Components

#### Layer 0 (Raw Storage)

**Pallet (`crates/pallets/storage-provider/`)**: On-chain logic for provider registration, bucket creation, storage agreements, checkpoints, and challenge/slashing mechanism.

**Runtime (`runtimes/web3-storage-local/`)**: Parachain runtime that includes the storage provider pallet and configures its parameters (stake requirements, challenge periods, etc.).

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

**Smart Contracts (`precompiles/`, `examples/contracts/`)**: `pallet_revive` (PolkaVM-based smart contracts) is wired into both runtimes. Two custom precompiles expose the client-side bucket lifecycle and drive registry to Solidity contracts. The `StorageMarketplace.sol` example shows how a dApp buys storage on behalf of its users; `just sc-demo` runs the end-to-end PAPI test. Full design in [docs/drafts/smart-contracts.md](docs/drafts/smart-contracts.md).

#### Layer 1 (File System Interface)

**File System Primitives (`storage-interfaces/file-system/primitives/`)**: High-level types for file system:
- `DriveInfo`: Drive metadata and configuration
- `DirectoryNode`: Protobuf-based directory structure
- `FileManifest`: File metadata with chunk tracking
- `CommitStrategy`: Checkpoint strategies (Immediate, Batched, Manual)
- Helper functions for CID computation and path handling

**Drive Registry Pallet (`crates/pallets/drive-registry/`)**: On-chain drive management:
- Drive creation with automatic infrastructure setup
- Root CID tracking for drive state
- User-to-drive mapping
- Bucket-to-drive mapping
- Drive lifecycle (create, update, clear, delete)

**File System Client (`storage-interfaces/file-system/client/`)**: High-level SDK providing:
- Familiar file/folder interface over Layer 0 blob storage
- Automatic drive creation and provider selection
- Directory operations (create, list, navigate)
- File operations (upload, download, delete)
- Real blockchain integration using `subxt`
- Content-addressed storage with CID verification
- Flexible commit strategies

**Example:** `storage-interfaces/file-system/client/examples/basic_usage.rs`
- Complete workflow: drive creation → directories → file uploads/downloads
- Real blockchain integration with event extraction
- Demonstrates the full Layer 1 capabilities

## Development Workflow

### Quick Start
1. **Setup**: `just setup` (one-time, downloads binaries and builds)
2. **Start**: `just start-chain` then `just start-provider` (in separate terminals)
3. **Configure**: with chain + provider running, `just demo` registers the provider, opens an agreement, and exercises challenges end-to-end (it does not start the chain or provider for you)
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

### Runtime Parameters (runtimes/web3-storage-local/src/storage.rs)

All durations are measured in **anchor (relay-chain) blocks** (`RC_HOURS`,
6 s each), not parachain blocks — the pallet reads its clock from
`Config::BlockNumberProvider` (see the anchor-clock section in
[docs/design/scalable-web3-storage-implementation.md](docs/design/scalable-web3-storage-implementation.md)):

```rust
// Token decimals
pub const UNIT: Balance = 1_000_000_000_000; // 12 decimals

// Minimum provider stake: 1000 tokens
pub const MinProviderStake: Balance = 1_000 * UNIT;

// 1 token (1e12) per 1 GB (1e9 bytes) = 1000 per byte
pub const MinStakePerByte: Balance = 1_000;

// Challenge response deadline (provider must respond within this many anchor blocks)
pub const ChallengeTimeout: BlockNumber = 48 * RC_HOURS;
pub const SettlementTimeout: BlockNumber = 24 * RC_HOURS;
pub const RequestTimeout: BlockNumber = 6 * RC_HOURS;

// Provider-initiated checkpoint config
pub const DefaultCheckpointInterval: BlockNumber = 100; // anchor blocks (~10 min)
pub const DefaultCheckpointGrace: BlockNumber = 20;     // anchor blocks (~2 min)
pub const CheckpointReward: Balance = 1_000_000_000_000;     // 1 token
pub const CheckpointMissPenalty: Balance = 500_000_000_000;  // 0.5 token
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
    max_capacity: u64,                // Maximum storage capacity (0 = unlimited)
}
```

### Capacity & Stake Requirements

Providers must stake tokens proportional to their declared capacity:

```rust
// Minimum stake per byte of declared capacity (1 token per GB)
pub const MinStakePerByte: Balance = 1_000;

// Required stake calculation
required_stake = max_capacity * MinStakePerByte

// Example: 1 TB capacity requires ~1.1e15 units (~1100 tokens) stake
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

## Advanced Features

### Provider Discovery & Marketplace

The SDK provides automatic provider discovery based on storage requirements:

```rust
use storage_client::{DiscoveryClient, StorageRequirements};

let mut client = DiscoveryClient::with_defaults()?;
client.connect().await?;

// Define requirements
let requirements = StorageRequirements {
    bytes_needed: 10 * 1024 * 1024 * 1024, // 10 GB
    min_duration: 100_000,
    max_price_per_byte: 1_000_000,
    primary_only: true,
};

// Find matching providers (sorted by score)
let providers = client.find_providers(requirements, 10).await?;

// Or get recommendations with cost estimates
let recommendations = client.suggest_providers(bytes, duration, budget).await?;
```

**Matching Algorithm**: Providers are scored 0-100 based on:
- Accepting status (not accepting = 0)
- Capacity (insufficient = -50 points)
- Price (too high = -30 points)
- Duration (mismatch = -20 points)

See [Storage Marketplace Design](docs/drafts/marketplace.md) for details.

### Checkpoint Management

The client SDK provides comprehensive checkpoint management:

```rust
use storage_client::{CheckpointManager, CheckpointConfig, BatchedCheckpointConfig};

// Create checkpoint manager
let manager = CheckpointManager::new(chain_endpoint, CheckpointConfig::default()).await?;
let manager = manager.with_providers(provider_endpoints);

// Manual checkpoint submission
let result = manager.submit_checkpoint(bucket_id).await;

// Or enable automatic checkpoints
let config = BatchedCheckpointConfig {
    interval: BatchedInterval::Blocks(100),
    ..Default::default()
};
let handle = manager.start_checkpoint_loop(bucket_id, config, callback).await?;

// Control the loop
handle.submit_now().await?;  // Force immediate checkpoint
handle.stop().await?;         // Stop background loop
```

**Key Components**:
- `CheckpointManager`: Coordinates multi-provider checkpoint collection and consensus
- `CheckpointPersistence`: Persists checkpoint state to disk with backup rotation
- `EventSubscriber`: Real-time blockchain event monitoring (checkpoints, challenges)
- `ProviderHealthHistory`: Tracks provider reliability and response times

See [Checkpoint Protocol Design](docs/drafts/CHECKPOINT_PROTOCOL.md) for details.

### Event Subscription

Subscribe to real-time blockchain events:

```rust
use storage_client::{EventSubscriber, EventFilter, StorageEvent};

let subscriber = EventSubscriber::connect(chain_endpoint).await?;

// Subscribe to specific events
let filter = EventFilter::bucket(bucket_id);
let mut stream = subscriber.subscribe(filter).await?;

while let Some(event) = stream.next().await {
    match event {
        StorageEvent::BucketCheckpointed { bucket_id, mmr_root, .. } => { /* ... */ }
        StorageEvent::ChallengeCreated { challenge_id, .. } => { /* ... */ }
        StorageEvent::ProviderSlashed { provider, amount, .. } => { /* ... */ }
        _ => {}
    }
}
```

## Code Review Guidelines (Parity Standards)

For the full review criteria (Parity Standards), see the `/review` skill. The review bot and all contributors follow those guidelines.

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
| [Layer 1 Quick Start](docs/getting-started/LAYER1_QUICKSTART.md) | Three-terminal setup + SDK examples |
| [Extrinsics Reference](docs/reference/EXTRINSICS_REFERENCE.md) | Complete blockchain API |
| [Payment Calculator](docs/reference/PAYMENT_CALCULATOR.md) | Calculate agreement costs |
| [Architecture Design](docs/design/scalable-web3-storage.md) | System design, economics, common concerns |
| [Implementation Details](docs/design/scalable-web3-storage-implementation.md) | Technical specs |
| [Execution Flows](docs/reference/EXECUTION_FLOWS.md) | Sequence diagrams for all extrinsics |
| [Storage Marketplace](docs/drafts/marketplace.md) | Provider capacity & discovery |
| [Checkpoint Protocol](docs/drafts/CHECKPOINT_PROTOCOL.md) | Automated checkpoint management |
| [File System Architecture](docs/filesystems/ARCHITECTURE.md) | Layer 1 encoding, security, blockchain details |

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
- With chain + provider already running, `just demo` performs that setup
- For chain health, run `bash scripts/check-chain.sh` (relay + parachain probe)

### Provider Not Accepting Agreements
- Call `updateProviderSettings` after registration
- Set `acceptingPrimary: true`

### "CapacityExceeded" or "InsufficientStakeForCapacity" Error
- Provider's `max_capacity` is too low for the agreement
- Or provider's stake doesn't cover their declared capacity
- Required: `stake >= max_capacity * MinStakePerByte`
- Use `DiscoveryClient.find_providers()` to find providers with sufficient capacity

## Feature Flags

- `runtime-benchmarks` - Enable weight generation
- `try-runtime` - Runtime migration testing
- `std` - Standard library features (default)

## Notes

- Token decimals: 12 (like Polkadot)
- Minimum stake: 1000 tokens
- Challenge response window: 48h (`48 * RC_HOURS` anchor blocks)
- All on-chain durations are anchor (relay-chain) blocks, 6 s each
- Data is content-addressed with blake2-256
- All data operations happen off-chain via HTTP
- Chain is only for accountability and disputes

## Using the Claude Review Bot

- **@claude** - Mention in any comment to ask questions or request help
- **Assign to claude[bot]** - Assign an issue to have Claude analyze and propose solutions
- **Label with `claude`** - Add the `claude` label to an issue for Claude to investigate
