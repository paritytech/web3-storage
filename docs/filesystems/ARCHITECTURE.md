# File System Architecture

## Table of Contents

1. [Overview](#overview)
2. [System Layers](#system-layers)
3. [Data Encoding & Serialization](#data-encoding--serialization)
4. [Content Addressing & CIDs](#content-addressing--cids)
5. [Security Model](#security-model)
6. [Encryption & Access Control](#encryption--access-control)
7. [Blockchain Integration](#blockchain-integration)
8. [Design Decisions](#design-decisions)
9. [Performance Considerations](#performance-considerations)
10. [API Documentation Links](#api-documentation-links)

---

## Overview

The File System Interface (Layer 1) provides a high-level abstraction over Scalable Web3 Storage (Layer 0), enabling users to work with familiar file system concepts while benefiting from decentralized, content-addressed storage with blockchain accountability.

```
┌─────────────────────────────────────────────────────────────────────┐
│  User Applications                                                   │
│  (Web apps, CLI tools, FUSE mounts)                                 │
└─────────────────────────────────────────────────────────────────────┘
                              ▲
                              │ File System Client SDK
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 1: File System Interface                                      │
│  ┌──────────────┐  ┌────────────────┐  ┌─────────────────┐         │
│  │ Drive        │  │ File System    │  │ File System     │         │
│  │ Registry     │  │ Primitives     │  │ Client SDK      │         │
│  │ (On-Chain)   │  │ (Types)        │  │ (Off-Chain)     │         │
│  └──────────────┘  └────────────────┘  └─────────────────┘         │
└─────────────────────────────────────────────────────────────────────┘
                              ▲
                              │ Bucket/Agreement APIs
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 0: Scalable Web3 Storage                                      │
│  ┌──────────────┐  ┌────────────────┐  ┌─────────────────┐         │
│  │ Storage      │  │ Provider       │  │ Storage         │         │
│  │ Pallet       │  │ Node           │  │ Client          │         │
│  │ (On-Chain)   │  │ (Off-Chain)    │  │ (Off-Chain)     │         │
│  └──────────────┘  └────────────────┘  └─────────────────┘         │
└─────────────────────────────────────────────────────────────────────┘
```

---

## System Layers

### Layer 0: Scalable Web3 Storage (Foundation)

**Purpose**: Provides raw blob storage with game-theoretic guarantees.

**Components**:
- **Storage Pallet**: On-chain logic for buckets, agreements, checkpoints, and challenges
- **Provider Node**: Off-chain HTTP server storing data chunks and building MMR commitments
- **Storage Client**: SDK for bucket operations, uploads, downloads, and verification

**Key Concepts**:
- **Buckets**: Logical containers for data with associated provider agreements
- **Agreements**: Contracts between users and providers specifying storage terms
- **Checkpoints**: Cryptographic commitments (MMR roots) submitted on-chain
- **Challenges**: Mechanism for verifying provider data integrity

### Layer 1: File System Interface (Abstraction)

**Purpose**: Provides familiar file/folder interface over Layer 0's content-addressed blob storage.

**Components**:
- **Drive Registry Pallet**: On-chain drive metadata and root CID tracking
- **File System Primitives**: Shared types (DirectoryNode, FileManifest, CommitStrategy)
- **File System Client**: High-level SDK for file/directory operations

**Key Concepts**:
- **Drives**: User's logical file systems backed by Layer 0 buckets
- **Root CID**: Content identifier of the root directory (stored on-chain)
- **Directory Nodes**: Protobuf/SCALE-encoded directory structures
- **File Manifests**: Metadata tracking file chunks

### Parachain Integration

Both Layer 0 and Layer 1 operate on the **same parachain**:

```
┌─────────────────────────────────────────────────────────────────────┐
│  Storage Parachain (ID: 4000)                                        │
│                                                                      │
│  ┌─────────────────────────────┐  ┌─────────────────────────────┐  │
│  │ pallet-storage-provider     │  │ pallet-drive-registry       │  │
│  │ (Layer 0)                   │  │ (Layer 1)                   │  │
│  │                             │  │                             │  │
│  │ - Buckets                   │  │ - Drives                    │  │
│  │ - Agreements                │  │ - Root CIDs                 │  │
│  │ - Checkpoints               │  │ - User registry             │  │
│  │ - Challenges                │  │                             │  │
│  └─────────────────────────────┘  └─────────────────────────────┘  │
│                                                                      │
│  Cross-Pallet Calls: DriveRegistry → StorageProvider                │
│  (create_bucket, request_agreement, end_agreement)                  │
└─────────────────────────────────────────────────────────────────────┘
        │
        │ Cumulus (Parachain Protocol)
        ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Relay Chain (Polkadot/Paseo)                                        │
│  - Shared security                                                   │
│  - Finality                                                          │
│  - Cross-chain messaging (future)                                    │
└─────────────────────────────────────────────────────────────────────┘
```

**Why Same Parachain?**

1. **Lower Latency**: Cross-pallet calls are atomic and synchronous
2. **Simpler Architecture**: No XCM messaging complexity
3. **Shared State**: Direct access to Layer 0 storage (buckets, agreements)
4. **Cost Efficiency**: Single transaction for drive creation + bucket setup

---

## Data Encoding & Serialization

The system uses two encoding formats depending on the context:

### SCALE Encoding (On-Chain & Content-Addressed Storage)

**Usage**:
- All on-chain storage (pallet state)
- Content-addressed data stored via providers
- CID computation base

**Why SCALE?**
- Substrate-native encoding (required for pallets)
- Deterministic: Same data always produces same bytes
- Efficient: Compact binary representation
- `no_std` compatible: Works in runtime WASM

**Format Details**:

```rust
// DirectoryNode SCALE encoding
struct DirectoryNode {
    drive_id: u64,                                    // 8 bytes, little-endian
    children: BoundedVec<DirectoryEntry, Max1024>,    // Length prefix + entries
    metadata: BoundedVec<MetadataEntry, Max64>,       // Length prefix + entries
}

// DirectoryEntry SCALE encoding
struct DirectoryEntry {
    name: BoundedVec<u8, Max256>,    // Length prefix + UTF-8 bytes
    entry_type: EntryType,            // 1 byte (0=File, 1=Directory)
    cid: H256,                        // 32 bytes (blake2-256 hash)
    size: u64,                        // 8 bytes, little-endian
    mtime: u64,                       // 8 bytes, Unix timestamp
}
```

**Example**: Empty DirectoryNode for drive_id=2

```
Bytes:   02 00 00 00 00 00 00 00  00  00
         └─────── drive_id ───────┘  └── children (empty vec)
                                        └── metadata (empty vec)
Length: 10 bytes
CID: 0xe835d9bb4ac2c42bd8895fcfb159903f4ce6de8de863182f4fb87c06a23d18b7
```

### Protobuf Encoding (Optional Off-Chain)

**Usage**:
- Client-side caching (optional)
- Inter-service communication
- Human-readable debugging

**Why Protobuf?**
- Self-describing schema
- Language-agnostic
- Better tooling for inspection

**Important**: Protobuf is **NOT** used for CID computation. CIDs are always computed from SCALE-encoded bytes to ensure consistency.

### Encoding Workflow

```
┌─────────────────────────────────────────────────────────────────────┐
│  Client Operations                                                   │
│                                                                      │
│  1. Create DirectoryNode struct                                      │
│  2. Serialize to SCALE: node.to_scale_bytes()                       │
│  3. Compute CID: blake2_256(scale_bytes)                            │
│  4. Upload SCALE bytes to provider (by CID)                         │
│  5. Store CID on-chain (root_cid)                                   │
│                                                                      │
│  Retrieval:                                                          │
│  1. Read root_cid from chain                                         │
│  2. Fetch SCALE bytes from provider (by CID)                        │
│  3. Verify: blake2_256(bytes) == expected_cid                       │
│  4. Deserialize: DirectoryNode::from_scale_bytes(&bytes)            │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Content Addressing & CIDs

### CID Format

Content Identifiers (CIDs) are 32-byte blake2-256 hashes:

```rust
pub type Cid = H256;  // sp_core::H256

pub fn compute_cid(data: &[u8]) -> Cid {
    sp_core::hashing::blake2_256(data).into()
}
```

### Why blake2-256?

1. **Substrate Standard**: Native hashing function in Substrate
2. **Performance**: Faster than SHA-256 while equally secure
3. **Collision Resistance**: 256-bit output provides strong guarantees
4. **Hardware Support**: Optimized implementations available

### Content-Addressed DAG

Files and directories form a Merkle DAG (Directed Acyclic Graph):

```
                    Root CID (on-chain)
                         │
                    ┌────┴────┐
                    │         │
               documents/   images/
                    │         │
              ┌─────┴─────┐   │
              │           │   │
          work/     notes.txt photo.jpg
              │
          report.txt

Each node's CID = blake2_256(SCALE_bytes)
Parent nodes contain children's CIDs
```

### Deduplication

Same content always produces same CID, enabling automatic deduplication:

```rust
// Two identical files
let file1_data = b"Hello, World!";
let file2_data = b"Hello, World!";

let cid1 = compute_cid(file1_data);  // 0xabc...
let cid2 = compute_cid(file2_data);  // 0xabc... (same!)

// Only stored once on provider
```

---

## Security Model

### Trust Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│  Trust Levels                                                       │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │ TRUSTLESS: Blockchain                                        │  │
│  │ - Finalized state is immutable                               │  │
│  │ - Consensus guarantees                                       │  │
│  │ - Root CIDs are verifiable                                   │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │ VERIFIABLE: Content-Addressed Storage                        │  │
│  │ - Data integrity verified by CID                             │  │
│  │ - Cannot serve tampered data                                 │  │
│  │ - Providers economically incentivized                        │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │ ACCOUNTABLE: Provider Network                                │  │
│  │ - Staked providers face slashing                             │  │
│  │ - Challenge mechanism for disputes                           │  │
│  │ - Replication for redundancy                                 │  │
│  └─────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────┘
```

### Data Integrity Verification

Every data retrieval is verified:

```rust
async fn fetch_blob(&self, cid: Cid) -> Result<Vec<u8>> {
    // 1. Fetch data from provider
    let data = self.storage_client.read(&cid, 0, length).await?;

    // 2. Provider verifies chunk hashes during read
    // (see storage-client/src/lib.rs lines 221-227)

    // 3. Client verifies entire blob CID
    let actual_cid = compute_cid(&data);
    if actual_cid != cid {
        return Err(Error::IntegrityCheckFailed);
    }

    Ok(data)
}
```

### Provider Accountability

```
┌────────────────────────────────────────────────────────────────────┐
│  Game-Theoretic Guarantees                                          │
│                                                                     │
│  Provider Registration:                                             │
│  - Minimum stake: 1000 tokens                                       │
│  - Stake locked during active agreements                            │
│                                                                     │
│  Checkpoint Flow:                                                   │
│  1. Provider builds MMR over stored data                            │
│  2. Provider signs commitment (MMR root)                            │
│  3. Client submits checkpoint on-chain                              │
│  4. Provider is now liable for data availability                    │
│                                                                     │
│  Challenge Mechanism:                                               │
│  1. Challenger requests proof for specific chunk                    │
│  2. Provider must respond within challenge_period                   │
│  3. Failure to respond → slashing (lose stake)                      │
│  4. Successful response → challenger pays challenge fee             │
│                                                                     │
│  Result: Providers economically motivated to preserve data          │
└────────────────────────────────────────────────────────────────────┘
```

### Access Control

**Current State**: Basic owner-based access

```rust
// Only drive owner can modify
fn update_root_cid(origin, drive_id, new_root_cid) {
    let caller = ensure_signed(origin)?;
    let drive = Drives::<T>::get(drive_id)?;
    ensure!(drive.owner == caller, Error::NotDriveOwner);
    // ... proceed with update
}
```

**Future Enhancements**: See [Encryption & Access Control](#encryption--access-control)

---

## Encryption & Access Control

### Current State

**Encryption is NOT implemented by default**. Data is stored in plaintext.

The system provides infrastructure for future encryption:

```rust
pub struct FileManifest {
    // ... other fields
    /// Encryption parameters (optional, for W3ACL)
    pub encryption_params: BoundedVec<u8, MaxEncryptionParamsLength>,  // 512 bytes max
}
```

### Planned Encryption Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│  Client-Side Encryption (Planned)                                   │
│                                                                     │
│  Upload:                                                            │
│  1. Generate random encryption key (AES-256-GCM)                    │
│  2. Encrypt file chunks with key                                    │
│  3. Encrypt key with owner's public key                             │
│  4. Store encrypted_key in FileManifest.encryption_params           │
│  5. Upload encrypted chunks                                         │
│                                                                     │
│  Download:                                                          │
│  1. Fetch FileManifest                                              │
│  2. Decrypt key with owner's private key                            │
│  3. Fetch and decrypt chunks                                        │
│                                                                     │
│  Sharing:                                                           │
│  1. Decrypt key with owner's private key                            │
│  2. Re-encrypt key with recipient's public key                      │
│  3. Create access grant (UCAN or W3ACL)                             │
└────────────────────────────────────────────────────────────────────┘
```

### Access Control Roadmap

| Feature | Status | Description |
|---------|--------|-------------|
| Owner-only access | Implemented | Drive owner can read/write |
| Client-side encryption | Planned | AES-256-GCM per file |
| UCAN delegation | Planned | Capability-based access tokens |
| W3ACL integration | Planned | Decentralized access control lists |
| Shared drives | Planned | Multi-user drive access |

### Security Recommendations

**For Sensitive Data (Current Workaround)**:

```rust
// Encrypt before upload
let key = generate_aes_key();
let encrypted_data = aes_gcm_encrypt(&file_data, &key);
let nonce = get_nonce_from_encryption();

// Store key securely (e.g., in your app's keystore)
fs_client.upload_file(drive_id, "/secret.enc", &encrypted_data, bucket_id).await?;

// Decrypt after download
let encrypted = fs_client.download_file(drive_id, "/secret.enc").await?;
let plaintext = aes_gcm_decrypt(&encrypted, &key, &nonce);
```

---

## Blockchain Integration

### Subxt Connection

The File System Client uses `subxt` for trustless blockchain interaction:

```rust
pub struct SubstrateClient {
    api: OnlineClient<SubstrateConfig>,
    signer: Option<Keypair>,
}

impl SubstrateClient {
    pub async fn connect(endpoint: &str) -> Result<Self> {
        // Connect to parachain WebSocket
        let api = OnlineClient::from_url(endpoint).await?;
        Ok(Self { api, signer: None })
    }
}
```

### Transaction Flow

```
┌────────────────────────────────────────────────────────────────────┐
│  Drive Creation Transaction Flow                                    │
│                                                                     │
│  1. Client builds extrinsic:                                        │
│     DriveRegistry::create_drive(name, capacity, period, payment)    │
│                                                                     │
│  2. Client signs with SR25519 keypair                               │
│                                                                     │
│  3. Submit to parachain:                                            │
│     POST /transaction                                               │
│                                                                     │
│  4. Transaction included in block                                   │
│                                                                     │
│  5. Client watches for finalization:                                │
│     - Poll transaction status                                       │
│     - Wait for finality (relay chain confirmation)                  │
│                                                                     │
│  6. Extract drive_id from DriveCreated event                        │
│                                                                     │
│  7. Query drive state:                                              │
│     DriveRegistry::Drives(drive_id) -> DriveInfo                    │
└────────────────────────────────────────────────────────────────────┘
```

### Storage Queries

```rust
// Query drive info
async fn query_drive_root_cid(&self, drive_id: DriveId) -> Result<Cid> {
    // Build storage key: twox128("DriveRegistry") + twox128("Drives") + blake2_128(drive_id)
    let storage_key = build_storage_key("DriveRegistry", "Drives", drive_id);

    // Fetch raw bytes from chain state
    let bytes = self.api.storage().at_latest().await?.fetch_raw(storage_key).await?;

    // Decode DriveInfo and extract root_cid
    let drive_info = decode_drive_info(&bytes)?;
    Ok(drive_info.root_cid)
}
```

### Event Extraction

```rust
// Find DriveCreated event after transaction
for event in events.iter() {
    if event.pallet_name() == "DriveRegistry" {
        if let Ok(value) = event.field_values() {
            // DriveCreated { drive_id, owner, bucket_id, root_cid }
            if let Some(drive_id) = value.at(0).and_then(|v| v.as_u128()) {
                return Ok(drive_id as DriveId);
            }
        }
    }
}
```

---

## Design Decisions

### Why SCALE over Protobuf for Storage?

| Aspect | SCALE | Protobuf |
|--------|-------|----------|
| Determinism | Guaranteed | Field order dependent |
| CID Stability | Always same for same data | Schema changes break CIDs |
| Substrate Integration | Native | Requires conversion |
| `no_std` Support | Yes | Requires `prost` with alloc |
| Size | Compact | Slightly larger |

**Decision**: Use SCALE for all stored data to ensure CID consistency.

### Why Same Parachain for L0 and L1?

**Alternatives Considered**:

1. **Separate Parachains**: L0 and L1 on different parachains
   - Pro: Independent scaling
   - Con: XCM complexity, latency, higher costs

2. **L1 on Relay Chain**: Drive registry on relay chain
   - Pro: Higher security
   - Con: Limited functionality, high costs

3. **Same Parachain** (Chosen):
   - Pro: Simple cross-pallet calls, shared state, low latency
   - Con: Coupled scaling

**Rationale**: Simplicity wins. File system operations frequently need bucket/agreement data. Cross-pallet calls are atomic and free.

### Why blake2-256 for CIDs?

**Alternatives**:
- SHA-256: Slower, no substrate optimization
- Keccak-256: Ethereum-compatible but not Substrate-native
- BLAKE3: Newer, not yet in Substrate

**Decision**: blake2-256 is Substrate-native, fast, and battle-tested.

### Why Content-Addressed Storage?

**Benefits**:
1. **Integrity**: CID = fingerprint of content
2. **Deduplication**: Same content stored once
3. **Immutability**: CIDs never change
4. **Verifiability**: Anyone can verify data integrity
5. **Caching**: Safe to cache forever

**Trade-off**: Updates create new CIDs, requiring DAG updates.

### Why Merkle DAG for Directories?

**Benefits**:
1. **Efficient Updates**: Only changed nodes need re-upload
2. **Versioning**: Each root CID is a complete snapshot
3. **Partial Sync**: Download only needed branches
4. **Proof of Inclusion**: Merkle proofs for any entry

### Commit Strategies: Cost vs. Latency

The on-chain root CID is updated according to the drive's `CommitStrategy`. Each change to the directory tree produces a new candidate root CID off-chain; the strategy decides when to actually write it on-chain.

| Strategy | Behavior | Tx cost | Latency | Use case |
|----------|----------|---------|---------|----------|
| `Immediate` | Every file/dir change → on-chain tx | High (1 tx per op) | None | Audit trails, regulatory data |
| `Batched { interval: N blocks }` | Coalesce changes; flush every N blocks | Medium (~1 tx per ~10 min @ N=100) | Up to N blocks | Default; collaborative documents |
| `Manual` | Stay off-chain until caller invokes `commit_changes` | Low (1 tx per N writes) | Caller-controlled | Bulk uploads, git-style workflows |

Batching trades freshness for cost. With `Batched { interval: 100 }`, a 100-file upload becomes one on-chain tx instead of 100; recovery still works because the off-chain pending state is reconstructable from the directory chunks the provider already holds.

### Why 1 Bucket = 1 Drive

Layer 1 deliberately does *not* invent its own access-control or pool primitive. Each drive maps to exactly one Layer-0 bucket (`pallet-drive-registry::BucketToDrive: u64 → DriveId`).

This means:

- **Permissions reuse Layer-0 bucket membership.** Layer 0 buckets already have `Admin` / `Reader` / `Writer` roles. Adding `Reader + Writer` to a bucket is what grants a user the ability to use a drive built on it. No separate "drive ACL" exists.
- **Admin / user separation comes for free.** The bucket admin manages infrastructure (creates the bucket, requests provider agreements, replaces failed providers, monitors challenges). The bucket member uses it as a drive (uploads, downloads, directory ops). The split is the bucket-membership split.
- **No new on-chain concepts.** An earlier design proposed a `StoragePool` abstraction with its own capacity, pricing, and access list; it was rejected because the bucket already carries all of that. The current registry stores `DriveInfo` and `BucketToDrive` and nothing else.

---

## Performance Considerations

### On-Chain Storage Footprint

Per drive, the registry stores a `DriveInfo` plus a 1:1 `BucketToDrive` mapping:

| Item | Size |
|------|-----:|
| `DriveInfo` (owner + bucket_id + root_cid + name + timestamps) | ~200 bytes |
| `BucketToDrive` map entry | 16 bytes |
| `UserDrives` index entry | 8 bytes |
| **Total per drive** | **~225 bytes** |

Cost scales linearly with drive count, not with file count or capacity. Files and directories live entirely off-chain in the provider's content-addressed storage; only the root CID moves on-chain on commit.

### Read Path Optimization

```
┌────────────────────────────────────────────────────────────────────┐
│  Read Path: download_file("/documents/report.pdf")                  │
│                                                                     │
│  1. Check root_cid cache (in-memory)                               │
│     └─ Hit: Skip chain query                                       │
│     └─ Miss: Query chain, cache result                             │
│                                                                     │
│  2. Traverse path: / → documents → report.pdf                       │
│     └─ Each step: Fetch directory node from provider                │
│     └─ Optimization: Batch fetches, prefetch siblings              │
│                                                                     │
│  3. Fetch file manifest                                             │
│                                                                     │
│  4. Fetch chunks in parallel                                        │
│     └─ Provider supports range requests                             │
│     └─ Client reassembles locally                                   │
│                                                                     │
│  Typical latency:                                                   │
│  - Cache hit: ~50ms (single provider round-trip)                   │
│  - Cache miss: ~200ms (chain query + provider)                     │
│  - Large file: Dominated by chunk download time                    │
└────────────────────────────────────────────────────────────────────┘
```

### Write Path Optimization

```
┌────────────────────────────────────────────────────────────────────┐
│  Write Path: upload_file("/documents/report.pdf", data)             │
│                                                                     │
│  1. Split file into 256 KiB chunks                                  │
│                                                                     │
│  2. Upload chunks in parallel                                       │
│     └─ Each chunk: Compute CID, upload to provider                  │
│     └─ Provider stores: CID → data                                  │
│                                                                     │
│  3. Create FileManifest with chunk CIDs                             │
│     └─ Upload manifest, get manifest CID                            │
│                                                                     │
│  4. Update parent directory                                         │
│     └─ Fetch current directory                                      │
│     └─ Add entry: name → manifest CID                               │
│     └─ Upload new directory, get new CID                            │
│                                                                     │
│  5. Update ancestors up to root                                     │
│     └─ Recursive: Each parent gets new CID                          │
│                                                                     │
│  6. Update on-chain root_cid                                        │
│     └─ Based on CommitStrategy:                                     │
│        - Immediate: Submit transaction now                          │
│        - Batched: Queue, submit on interval                         │
│        - Manual: Store pending, wait for commit_changes()           │
│                                                                     │
│  Optimization: Batch multiple writes before chain update            │
└────────────────────────────────────────────────────────────────────┘
```

### Provider API Read Limits

**Important**: When reading data from providers, avoid `u64::MAX` as length parameter:

```rust
// BAD: Causes overflow in provider's chunk calculation
let data = storage_client.read(&cid, 0, u64::MAX).await?;

// GOOD: Use reasonable maximum (1 TiB)
const MAX_READ_LENGTH: u64 = 1024 * 1024 * 1024 * 1024;
let data = storage_client.read(&cid, 0, MAX_READ_LENGTH).await?;
```

**Reason**: Provider calculates `end_chunk = (offset + length + chunk_size - 1) / chunk_size`. With `u64::MAX`, this overflows and returns no chunks.

---

## API Documentation Links

### User Documentation

| Document | Description |
|----------|-------------|
| [User Guide](./USER_GUIDE.md) | Complete guide for end users |

### Administrator Documentation

| Document | Description |
|----------|-------------|
| [Admin Guide](./ADMIN_GUIDE.md) | System administration and monitoring |

### Developer Documentation

| Document | Description |
|----------|-------------|
| [API Reference](./API_REFERENCE.md) | Complete API documentation |

### Layer 0 Documentation

| Document | Description |
|----------|-------------|
| [Extrinsics Reference](../reference/EXTRINSICS_REFERENCE.md) | Layer 0 blockchain API |
| [Payment Calculator](../reference/PAYMENT_CALCULATOR.md) | Calculate storage costs |
| [Layer 1 Quick Start](../getting-started/LAYER1_QUICKSTART.md) | Three-terminal setup + SDK examples |

### Design Documents

| Document | Description |
|----------|-------------|
| [Scalable Web3 Storage Design](../design/scalable-web3-storage.md) | System design & rationale |
| [Implementation Details](../design/scalable-web3-storage-implementation.md) | Technical specifications |

---

## Appendix: Encoding Examples

### DirectoryNode with Children

```rust
let dir = DirectoryNode {
    drive_id: 5,
    children: vec![
        DirectoryEntry {
            name: "documents",
            entry_type: Directory,
            cid: 0x9955e72d...,
            size: 0,
            mtime: 1707456000,
        },
        DirectoryEntry {
            name: "README.md",
            entry_type: File,
            cid: 0x0bc42ff7...,
            size: 127,
            mtime: 1707456020,
        },
    ],
    metadata: vec![],
};

// SCALE encoding (184 bytes for this example):
// 05 00 00 00 00 00 00 00    // drive_id: 5
// 0c                          // children count: 3 (compact)
// 24                          // name length: 9 (compact)
// 64 6f 63 75 6d 65 6e 74 73  // "documents"
// 01                          // entry_type: Directory
// 99 55 e7 2d ...             // cid: 32 bytes
// 00 00 00 00 00 00 00 00    // size: 0
// 05 08 28 96 90 00 00 00    // mtime
// ...
```

### FileManifest with Chunks

```rust
let manifest = FileManifest {
    drive_id: 5,
    mime_type: "application/pdf",
    total_size: 1048576,  // 1 MiB
    chunks: vec![
        FileChunk { cid: 0xabc..., sequence: 0 },
        FileChunk { cid: 0xdef..., sequence: 1 },
        FileChunk { cid: 0x123..., sequence: 2 },
        FileChunk { cid: 0x456..., sequence: 3 },
    ],
    encryption_params: vec![],  // Empty (no encryption)
};
```

---

## Glossary

| Term | Definition |
|------|------------|
| **CID** | Content Identifier - blake2-256 hash of data |
| **DAG** | Directed Acyclic Graph - tree structure of CIDs |
| **Drive** | User's logical file system (Layer 1 concept) |
| **Bucket** | Storage container (Layer 0 concept) |
| **MMR** | Merkle Mountain Range - efficient append-only commitment |
| **SCALE** | Simple Concatenated Aggregate Little-Endian encoding |
| **Checkpoint** | On-chain commitment to off-chain data state |
| **Root CID** | CID of the root directory (stored on-chain) |

---

*Last updated: February 2026*
