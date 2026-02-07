# File System Interface (Layer 1)

This directory contains the Layer 1 file system implementation built on top of Layer 0 (Scalable Web3 Storage).

Located in: `storage-interfaces/file-system/`

## User Experience: Truly Simplified Storage

Layer 1 File System provides a **true abstraction** over Layer 0. Users only need to understand **drives and files** - all infrastructure details are completely hidden!

### User Flow (Simple!)

```rust
// 1. Create a drive (specify storage needs)
let drive_id = fs_client.create_drive(
    Some("My Documents"),
    10_000_000_000,     // 10 GB storage
    500,                 // 500 blocks duration
    1_000_000_000_000,   // 1 token payment (12 decimals)
    None,                // Use default providers (auto-determined)
    None,                // Use default commit strategy (batched every 100 blocks)
).await?;

// 2. Use it like normal file storage!
fs_client.upload_file(drive_id, "/report.pdf", data).await?;
let entries = fs_client.list_directory(drive_id, "/").await?;
let data = fs_client.download_file(drive_id, "/report.pdf").await?;

// Advanced: Create drive with custom configuration
let drive_id = fs_client.create_drive(
    Some("Critical Data"),
    5_000_000_000,       // 5 GB
    2000,                // Long-term storage
    2_000_000_000_000,   // 2 tokens
    Some(5),             // 5 providers (1 primary + 4 replicas)
    Some(CommitStrategy::Immediate), // Real-time commits
).await?;
```

**What happens automatically (hidden from user):**
- ✅ System creates bucket in Layer 0
- ✅ System requests storage agreements with providers
- ✅ System sets up replication and redundancy
- ✅ System handles provider failures transparently

### Admin Flow (Monitoring & Policies)

Admins focus on system health rather than manual setup:

1. **Ensure Providers Available** - Monitor provider capacity and health
2. **Set System Policies** - Configure defaults (providers per drive, pricing, duration)
3. **Monitor System** - Track drives, storage usage, challenges
4. **Handle Failures** - Replace failed providers when needed

**See Examples:**
- `examples/user_workflow_simplified.rs` - User creating drives and managing files
- `examples/admin_workflow_simplified.rs` - Admin monitoring and management

**Key Benefits:**
- ✅ Users have ZERO knowledge of buckets, agreements, or providers
- ✅ Single API call to create a drive (vs 5-10 manual steps in Layer 0)
- ✅ System automates all infrastructure creation
- ✅ Admin burden reduced by 250× (monitoring vs manual setup)

## Architecture Overview

Following the three-layered architecture:

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 2: User Interfaces                                   │
│  (FUSE drivers, Web UI, CLI tools)                         │
│  [Future Work]                                              │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: File System Interface (THIS LAYER)                │
│                                                              │
│  ┌────────────────┐    ┌──────────────────────────────┐    │
│  │   Primitives   │    │   Pallet Registry            │    │
│  │                │    │                              │    │
│  │  - DirectoryNode   │  - create_drive()            │    │
│  │  - FileManifest    │  - update_root_cid()         │    │
│  │  - DriveInfo       │  - delete_drive()            │    │
│  │  - CID helpers     │  - Multi-drive per account   │    │
│  └────────────────┘    └──────────────────────────────┘    │
│                                                              │
│  Responsibilities:                                           │
│  - Metadata management (directories, files)                 │
│  - DAG navigation (Merkle-DAG traversal)                   │
│  - Drive registry (on-chain root CID tracking)             │
│  - Namespace & hierarchical structure                       │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
┌─────────────────────────────────────────────────────────────┐
│  Layer 0: Scalable Web3 Storage                            │
│  (Raw blob storage, buckets, agreements, challenges)       │
└─────────────────────────────────────────────────────────────┘
```

## Components

### `primitives/`
Core data structures and types for the file system.

**Key Types:**
- `DirectoryNode`: Protobuf-serialized directory with child references
- `FileManifest`: File metadata and chunk references
- `DriveInfo`: On-chain drive metadata (owner, bucket, root CID)
- `Cid`: Content identifier (blake2-256 hash)

**Features:**
- Protobuf schemas for efficient serialization
- CID computation and manipulation
- DAG helper functions

### `pallet-registry/`
On-chain registry pallet for drive management.

**User-Facing Extrinsics:**
- `create_drive(name, max_capacity, storage_period, payment, min_providers, commit_strategy)` - **[PRIMARY API]** Create drive (system auto-creates bucket and agreements)
  - `name`: Optional human-readable name
  - `max_capacity`: Maximum storage in bytes (e.g., 10 GB = 10_000_000_000)
  - `storage_period`: Duration in blocks (e.g., 500 blocks)
  - `payment`: Upfront payment tokens (e.g., 1_000_000_000_000 for 1 token with 12 decimals)
  - `min_providers`: Optional minimum number of providers (default: 3 for long-term [>1000 blocks], 1 for short-term)
  - `commit_strategy`: Optional checkpoint strategy (default: Batched every 100 blocks)
    - `Immediate`: Commit every change immediately (expensive but real-time)
    - `Batched { interval }`: Commit changes in batches after N blocks
    - `Manual`: User manually triggers commits via `commit_changes`
- `update_root_cid(drive_id, new_root_cid)` - Update after file system changes
- `commit_changes(drive_id)` - Commit pending changes (for batched/manual strategy)
- `delete_drive(drive_id)` - Remove drive
- `update_drive_name(drive_id, name)` - Rename drive

**Internal/Legacy Extrinsics:**
- `create_drive_with_bucket(bucket_id, root_cid, name)` - Low-level API for existing buckets (deprecated)
- `create_drive_with_storage(...)` - Old complex flow (deprecated)
- `raise_drive_dispute(...)` - Admin handles disputes at Layer 0 (deprecated)
- `replace_provider(...)` - Admin handles provider replacement at Layer 0 (deprecated)

**Storage:**
- `Drives: DriveId → DriveInfo` - Drive registry
- `UserDrives: AccountId → Vec<DriveId>` - User's drives
- `BucketToDrive: u64 → DriveId` - 1-to-1 bucket-drive mapping (internal)
- `NextDriveId: u64` - Auto-incrementing counter

**Automatic Behavior:**
The `create_drive` extrinsic automatically:
1. Creates a bucket in Layer 0 with specified capacity
2. Determines optimal number of providers:
   - If `min_providers` specified: uses that value
   - Otherwise: 3 (1 primary + 2 replicas) for periods > 1000 blocks, 1 provider for shorter periods
3. Automatically selects providers with sufficient capacity
4. Requests storage agreements with selected providers for the specified duration
5. Distributes payment equally across all providers
6. Configures checkpoint strategy (immediate, batched, or manual)
7. Creates empty drive structure
8. Returns drive_id to user

**Default Configuration:**
- **Replication**:
  - Short-term (<= 1000 blocks): 1 provider (primary only)
  - Long-term (> 1000 blocks): 3 providers (1 primary + 2 replicas)
  - Custom: Specify `min_providers` parameter
- **Checkpoints**: Batched every 100 blocks (customize with `commit_strategy`)
- **Provider selection**: Automatic based on availability and capacity
- Advanced users can customize bucket configuration via Layer 0 APIs directly

**Features:**
- Multi-drive support (multiple drives per account)
- Immutable versioning (each root CID = snapshot)
- Commit strategies (Immediate, Batched, Manual)
- Automatic infrastructure provisioning
- Transparent bucket management
- Event emission for all operations

## Data Flow

### Writing a File

```
1. Client splits file into chunks
   └─> Upload chunks to Layer 0 bucket

2. Client creates FileManifest
   └─> Serialize with protobuf
   └─> Upload to Layer 0 bucket (get file_cid)

3. Client updates parent DirectoryNode
   └─> Add DirectoryEntry { name: "file.txt", cid: file_cid, ... }
   └─> Serialize and upload (get new_parent_cid)

4. Client recursively updates parents up to root
   └─> Generate new root_cid

5. Client calls update_root_cid(drive_id, new_root_cid)
   └─> On-chain update (creates new snapshot)
```

### Reading a File

```
1. Query on-chain: get_drive_root_cid(drive_id)
   └─> Returns root_cid

2. Fetch from Layer 0: GET /node?hash=root_cid
   └─> Deserialize DirectoryNode (root /)

3. Traverse path: /documents/report.pdf
   └─> Find "documents" → get documents_cid
   └─> Fetch documents_cid → DirectoryNode
   └─> Find "report.pdf" → get report_cid

4. Fetch FileManifest: GET /node?hash=report_cid
   └─> Get list of chunk CIDs

5. Fetch and reconstruct file from chunks
```

## Design Decisions

### Why Names in Parent?
Storing entry names in the parent DirectoryNode (not in the child) optimizes renames:
- Renaming only changes parent blob
- Child CID stays stable (good for caching)
- Minimal cascade (only path from changed dir → root)

### Why Multi-Drive Per Account?
Flexibility for different use cases:
- Personal vs Work drives
- Public vs Private drives
- Different storage providers per drive
- Easier access control management

### Why Immutable Versioning?
Each root CID represents a complete snapshot:
- "Time machine" capability (access any historical state)
- Audit trail of all changes
- Easy rollback to previous versions
- Compatible with IPFS/IPLD patterns

## Usage Example

```rust
use file_system_primitives::{DirectoryNode, FileManifest, compute_cid};
use pallet_drive_registry::Pallet as DriveRegistry;

// Create empty drive
let root = DirectoryNode::new_empty("drive_1");
let root_cid = root.compute_cid()?;
let root_bytes = root.to_bytes()?;

// Upload root to Layer 0
provider_client.upload(bucket_id, &root_bytes).await?;

// Register drive on-chain
DriveRegistry::create_drive(
    origin,
    bucket_id,
    root_cid,
    Some(b"My Drive".to_vec())
)?;
```

## Testing

```bash
# Test primitives
cargo test -p file-system-primitives

# Test pallet
cargo test -p pallet-drive-registry

# Run all Layer 1 tests
cargo test -p file-system-primitives -p pallet-drive-registry
```

## Future Work

### Planned Features (File System Interface)
- [ ] Client SDK for high-level file operations
- [ ] DAG builder and traversal utilities
- [ ] Path resolution helpers
- [ ] Batch operations (multiple file changes → single root update)
- [ ] Indexer service (off-chain metadata indexing)
- [ ] Search API (full-text search on file names/metadata)

### Layer 2 Integration (Future)
- [ ] FUSE driver for local mounting
- [ ] Web dashboard (Google Drive-like UI)
- [ ] CLI tools (ls, cp, mv, rm)
- [ ] WebDAV server
- [ ] Access control (W3ACL/UCAN integration)

## References

- [Layer 1 Design Doc](../../docs/design/layer-1-file-system.md) _(to be created)_
- [Three-Layered Architecture](../../docs/design/scalable-web3-storage.md)
- [Layer 0 Implementation](../../docs/design/scalable-web3-storage-implementation.md)
- [Protobuf Schemas](./primitives/proto/filesystem.proto)

## Contributing

When adding new features to the File System Interface:
1. Keep Layer 0 dependencies minimal (only use primitives)
2. Follow the DAG/content-addressed pattern
3. Add comprehensive tests
4. Update this README with new components
5. Document in architecture docs
