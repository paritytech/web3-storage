# File System Interface (Layer 1)

This directory contains the Layer 1 file system implementation built on top of Layer 0 (Scalable Web3 Storage).

Located in: `storage-interfaces/file-system/`

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

**Extrinsics:**
- `create_drive(bucket_id, root_cid, name)` - Create new drive
- `update_root_cid(drive_id, new_root_cid)` - Update after file system changes
- `delete_drive(drive_id)` - Remove drive
- `update_drive_name(drive_id, name)` - Rename drive

**Storage:**
- `Drives: DriveId → DriveInfo` - Drive registry
- `UserDrives: AccountId → Vec<DriveId>` - User's drives
- `NextDriveId: u64` - Auto-incrementing counter

**Features:**
- Multi-drive support (multiple drives per account)
- Immutable versioning (each root CID = snapshot)
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
