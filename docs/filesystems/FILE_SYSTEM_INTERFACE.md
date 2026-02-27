# File System Interface (Layer 1)

## Overview

The File System Interface is Layer 1 of the Scalable Web3 Storage system, providing a high-level abstraction over Layer 0's raw blob storage. It enables users to work with familiar file system concepts (drives, directories, files) without needing to understand the underlying infrastructure (buckets, providers, agreements, challenges).

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 2: User Interfaces (Future)                          │
│  - FUSE drivers, Web UI, CLI tools                         │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: File System Interface (THIS LAYER)                │
│                                                              │
│  Components:                                                 │
│  - Drive Registry Pallet (on-chain)                        │
│  - File System Primitives (types & helpers)                │
│  - Client SDK (Rust library)                               │
│                                                              │
│  Capabilities:                                              │
│  - Drive creation with automatic infrastructure setup       │
│  - Directory & file operations                             │
│  - Versioning & snapshots                                  │
│  - Multi-drive management per account                      │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
┌─────────────────────────────────────────────────────────────┐
│  Layer 0: Scalable Web3 Storage                            │
│  - Buckets, Agreements, Providers, Challenges              │
└─────────────────────────────────────────────────────────────┘
```

## Key Concepts

### Drives
A **Drive** is a user's logical file system, similar to a disk partition or cloud storage folder. Each drive:
- Has a unique ID
- Is backed by a Layer 0 bucket
- Contains a hierarchical directory structure
- Tracks its root CID (content identifier)
- Supports versioning through immutable snapshots

**Properties:**
- `drive_id`: Unique identifier (u64)
- `owner`: Account that created the drive
- `bucket_id`: Associated Layer 0 bucket
- `root_cid`: Current root directory CID
- `name`: Optional human-readable name
- `max_capacity`: Maximum storage in bytes
- `storage_period`: Duration in blocks
- `expires_at`: Expiration block number
- `payment`: Total payment for storage
- `commit_strategy`: Checkpoint frequency

### Directory Structure
Files are organized in a hierarchical tree using **DirectoryNodes** and **FileManifests**:

```
Root Directory (CID: 0xabc...)
├── documents/ (CID: 0xdef...)
│   ├── report.pdf (CID: 0x123...)
│   └── presentation.pptx (CID: 0x456...)
└── images/ (CID: 0x789...)
    ├── photo1.jpg (CID: 0xaaa...)
    └── vacation/ (CID: 0xbbb...)
        └── beach.jpg (CID: 0xccc...)
```

Each node is content-addressed using blake2-256 hashing, enabling:
- Deduplication (same content = same CID)
- Integrity verification
- Efficient change detection
- Historical version tracking

### Commit Strategies
Control how frequently directory changes are committed to the blockchain:

| Strategy | Description | Use Case | Cost |
|----------|-------------|----------|------|
| **Immediate** | Every change commits immediately | Real-time collaboration, critical data | High (many transactions) |
| **Batched** | Commits every N blocks (default: 100) | Normal usage, balanced approach | Medium (periodic transactions) |
| **Manual** | User explicitly triggers commits | Batch operations, controlled checkpoints | Low (minimal transactions) |

### Provider Replication
Automatic provider selection based on storage duration:

| Duration | Default Providers | Redundancy Level |
|----------|------------------|------------------|
| Short-term (≤1000 blocks) | 1 provider | Single copy |
| Long-term (>1000 blocks) | 3 providers | 1 primary + 2 replicas |
| Custom | User-specified | Configurable |

## Capabilities Overview

### User Capabilities
✅ **Drive Management**
- Create drives with automatic infrastructure setup
- List all owned drives
- Rename drives
- Delete drives (when empty)

✅ **File Operations**
- Upload files (split into chunks automatically)
- Download files (reconstruct from chunks)
- Delete files
- List directory contents

✅ **Directory Operations**
- Create directories
- Navigate directory tree
- List subdirectories and files

✅ **Versioning**
- Access historical snapshots via root CIDs
- Roll back to previous versions
- Audit trail of all changes

✅ **Configuration**
- Customize storage capacity
- Set storage duration
- Choose replication level (provider count)
- Configure checkpoint frequency

### Admin Capabilities
✅ **System Monitoring**
- View all drives in the system
- Track storage usage and capacity
- Monitor provider health and availability
- Audit drive creation and modifications

✅ **Policy Management**
- Set default provider counts
- Configure default checkpoint strategies
- Set minimum storage requirements
- Define pricing policies (via Layer 0)

✅ **Provider Management**
- Register new storage providers
- Update provider settings
- Monitor provider performance
- Handle provider failures (replace providers)

✅ **Dispute Resolution**
- Monitor challenges (handled at Layer 0)
- Verify provider commitments
- Process slashing events
- Replace failed providers

## What Users DON'T Need to Know

The File System Interface completely abstracts away:
- ❌ Buckets (Layer 0 concept)
- ❌ Storage agreements
- ❌ Provider accounts and selection
- ❌ Challenges and proofs
- ❌ MMR (Merkle Mountain Range) commitments
- ❌ Payment distribution
- ❌ Checkpoint mechanics

**This is TRUE abstraction** - users work with drives and files, period.

## Comparison: With vs Without Layer 1

### Without Layer 1 (Direct Layer 0 Usage)
User must perform **10+ steps** to store a single file:
1. Create a bucket
2. Find available storage providers
3. Request primary agreement with provider 1
4. Request replica agreement with provider 2
5. Request replica agreement with provider 3
6. Wait for all providers to accept
7. Upload each file chunk manually
8. Create and manage directory Merkle-DAG
9. Track all CIDs manually
10. Handle provider failures manually

### With Layer 1 (File System Interface)
User performs **2 steps**:
1. **Create drive** → System automatically creates bucket and agreements
2. **Upload file** → System handles chunking, DAG, and CID tracking

**Complexity Reduction: 10 steps → 2 steps (80% simpler)**

## Use Cases

### Personal Storage
```rust
// Create a personal documents drive
let drive_id = fs_client.create_drive(
    Some("My Documents"),
    10_000_000_000,      // 10 GB
    500,                  // 500 blocks
    1_000_000_000_000,    // 1 token
    None,                 // Auto: 1 provider
    None,                 // Auto: batched commits
).await?;

// Upload documents
fs_client.upload_file(drive_id, "/resume.pdf", resume_data).await?;
fs_client.upload_file(drive_id, "/cover-letter.pdf", letter_data).await?;
```

### Long-Term Archive
```rust
// Create highly replicated archive
let drive_id = fs_client.create_drive(
    Some("Company Archive"),
    100_000_000_000,      // 100 GB
    10_000,               // Long-term (10k blocks)
    10_000_000_000_000,   // 10 tokens
    Some(5),              // 5 providers (high redundancy)
    None,                 // Batched commits (efficient)
).await?;
```

### Real-Time Collaboration
```rust
// Create drive with immediate commits
let drive_id = fs_client.create_drive(
    Some("Shared Project"),
    5_000_000_000,        // 5 GB
    1_000,                // 1000 blocks
    2_000_000_000_000,    // 2 tokens
    Some(3),              // 3 providers (standard redundancy)
    Some(CommitStrategy::Immediate), // Real-time updates
).await?;
```

## Documentation

- **[User Guide](./USER_GUIDE.md)** - Complete guide for end users
- **[Admin Guide](./ADMIN_GUIDE.md)** - System administration and monitoring
- **[API Reference](./API_REFERENCE.md)** - Complete API documentation
- **[Architecture Design](../design/layer-1-file-system.md)** - Technical architecture

## Related Documentation

- **[Layer 0 Design](../design/scalable-web3-storage.md)** - Underlying storage system
- **[Layer 0 Implementation](../design/scalable-web3-storage-implementation.md)** - Technical details
- **[Quick Start Guide](../getting-started/QUICKSTART.md)** - Get started quickly
- **[Testing Guide](../testing/MANUAL_TESTING_GUIDE.md)** - Testing procedures

## Technical Components

### On-Chain (Pallet)
- **Drive Registry**: Maps drive IDs to drive metadata
- **User Registry**: Maps accounts to their drives
- **Bucket Mapping**: 1-to-1 mapping between buckets and drives

### Off-Chain (Client SDK)
- **File Operations**: Upload, download, delete
- **Directory Management**: Create, navigate, list
- **DAG Builder**: Constructs Merkle-DAG from files
- **CID Cache**: Optimizes lookups

### Primitives (Shared Types)
- **DriveInfo**: Drive metadata structure
- **DirectoryNode**: Protobuf-serialized directory
- **FileManifest**: File metadata and chunk references
- **CommitStrategy**: Checkpoint configuration

## Future Enhancements

**Planned (Layer 1)**
- [ ] Batch operations (multiple file changes → single commit)
- [ ] Indexer service (off-chain metadata indexing)
- [ ] Search API (full-text search on file names)
- [ ] Path resolution helpers
- [ ] Symbolic links support

**Future (Layer 2)**
- [ ] FUSE driver for local mounting
- [ ] Web dashboard (Google Drive-like UI)
- [ ] CLI tools (ls, cp, mv, rm)
- [ ] WebDAV server
- [ ] Access control (W3ACL/UCAN integration)
- [ ] File sharing and permissions

## Getting Started

See the **[User Guide](./USER_GUIDE.md)** to start using the File System Interface.

For system administration, see the **[Admin Guide](./ADMIN_GUIDE.md)**.
