# File System Interface Documentation

Welcome to the File System Interface (Layer 1) documentation for Scalable Web3 Storage!

## What is the File System Interface?

The File System Interface is a **high-level abstraction** over Layer 0's raw blob storage, allowing users to work with familiar concepts like drives, directories, and files without worrying about the underlying decentralized infrastructure.

**Think of it as:**
- **Dropbox/Google Drive** but decentralized
- **IPFS** but with guaranteed storage and accountability
- **Traditional file system** but on blockchain

## Documentation Structure

### 📚 Core Documentation

| Document | Audience | Description |
|----------|----------|-------------|
| **[FILE_SYSTEM_INTERFACE.md](./FILE_SYSTEM_INTERFACE.md)** | Everyone | Architecture overview, capabilities, and use cases |
| **[USER_GUIDE.md](./USER_GUIDE.md)** | End Users | Complete guide for using the file system |
| **[ADMIN_GUIDE.md](./ADMIN_GUIDE.md)** | Administrators | System management and monitoring |
| **[API_REFERENCE.md](./API_REFERENCE.md)** | Developers | Complete API documentation |

## Quick Start

### For End Users

Start here: **[User Guide](./USER_GUIDE.md)**

**Quick Example:**
```rust
// 1. Create a drive (10 GB, 500 blocks)
let drive_id = fs_client.create_drive(
    Some("My Documents"),
    10_000_000_000,
    500,
    1_000_000_000_000,
    None,  // Auto-select providers
    None,  // Default commit strategy
).await?;

// 2. Upload files
fs_client.upload_file(drive_id, "/report.pdf", data, bucket_id).await?;

// 3. Download files
let data = fs_client.download_file(drive_id, "/report.pdf").await?;
```

### For Administrators

Start here: **[Admin Guide](./ADMIN_GUIDE.md)**

**Key Responsibilities:**
- Monitor provider health and capacity
- Set system policies and defaults
- Handle provider failures
- Track system metrics

### For Developers

Start here: **[API Reference](./API_REFERENCE.md)**

**Available APIs:**
- **On-Chain Extrinsics**: Drive creation, updates, deletion
- **Client SDK**: File/directory operations
- **Primitives**: Shared types and utilities

## Key Concepts

### 🗂️ Drives
A **drive** is your storage space. Each drive:
- Has a unique ID
- Is backed by a Layer 0 bucket
- Contains a hierarchical directory structure
- Supports versioning through immutable snapshots

### 📁 Directory Structure
Files are organized hierarchically using content-addressed nodes:
```
Root (CID: 0xabc...)
├── documents/
│   ├── report.pdf
│   └── notes.txt
└── images/
    └── photo.jpg
```

### ⏱️ Commit Strategies
Control when changes are saved:
- **Immediate**: Every change commits (real-time, expensive)
- **Batched**: Commits every N blocks (balanced, default: 100)
- **Manual**: User controls commits (efficient, batch operations)

### 🔄 Provider Replication
Automatic redundancy based on storage duration:
- Short-term (≤1000 blocks): 1 provider
- Long-term (>1000 blocks): 3 providers (1 primary + 2 replicas)
- Custom: User-specified count

## What Makes This Different?

### ❌ Without Layer 1 (Direct Layer 0)
Users must perform **10+ manual steps**:
1. Create a bucket
2. Find storage providers
3. Request primary agreement
4. Request replica agreements
5. Wait for acceptances
6. Upload chunks manually
7. Manage Merkle-DAG
8. Track all CIDs
9. Handle failures manually
10. Distribute payments

### ✅ With Layer 1 (File System Interface)
Users perform **2 simple steps**:
1. **Create drive** → System handles infrastructure
2. **Upload file** → System handles everything else

**Result: 80% complexity reduction**

## User Capabilities

✅ **Drive Management**
- Create drives with automatic setup
- List owned drives
- Rename/delete drives

✅ **File Operations**
- Upload files (auto-chunking)
- Download files (auto-reconstruction)
- Delete files

✅ **Directory Operations**
- Create directories
- Navigate directory tree
- List contents

✅ **Versioning**
- Access historical snapshots
- Roll back to previous versions
- Complete audit trail

✅ **Configuration**
- Customize storage capacity
- Set storage duration
- Choose replication level
- Configure checkpoint frequency

## Admin Capabilities

✅ **System Monitoring**
- View all drives
- Track storage usage
- Monitor provider health
- Audit operations

✅ **Policy Management**
- Set default provider counts
- Configure checkpoint strategies
- Define storage requirements
- Set pricing policies

✅ **Provider Management**
- Register providers
- Update provider settings
- Monitor performance
- Handle failures

✅ **Dispute Resolution**
- Monitor challenges
- Verify commitments
- Process slashing
- Replace failed providers

## Use Cases

### Personal Storage
```rust
// 10 GB drive with auto-defaults
let drive_id = fs_client.create_drive(
    Some("My Files"),
    10_000_000_000,
    500,
    1_000_000_000_000,
    None, None,
).await?;
```

### Long-Term Archive
```rust
// 100 GB with 5 providers for maximum redundancy
let drive_id = fs_client.create_drive(
    Some("Archive"),
    100_000_000_000,
    10_000,
    10_000_000_000_000,
    Some(5),  // High redundancy
    None,
).await?;
```

### Real-Time Collaboration
```rust
// Immediate commits for real-time updates
let drive_id = fs_client.create_drive(
    Some("Team Project"),
    5_000_000_000,
    1_000,
    2_000_000_000_000,
    Some(3),
    Some(CommitStrategy::Immediate),
).await?;
```

## Architecture

```
┌─────────────────────────────────────────┐
│  Layer 2: User Interfaces (Future)     │
│  - FUSE drivers, Web UI, CLI           │
└─────────────────────────────────────────┘
                   ▲
                   │
┌─────────────────────────────────────────┐
│  Layer 1: File System Interface         │
│  - Drive Registry (on-chain)            │
│  - File System Primitives               │
│  - Client SDK                           │
└─────────────────────────────────────────┘
                   ▲
                   │
┌─────────────────────────────────────────┐
│  Layer 0: Scalable Web3 Storage        │
│  - Buckets, Agreements, Providers      │
└─────────────────────────────────────────┘
```

## Component Overview

### On-Chain (Pallet)
**Location:** `storage-interfaces/file-system/pallet-registry/`

Substrate pallet managing:
- Drive registry (maps drive IDs to metadata)
- User registry (maps accounts to drives)
- Bucket-to-drive mapping
- Drive lifecycle (create, update, delete)

### Off-Chain (Client SDK)
**Location:** `storage-interfaces/file-system/client/`

Rust library providing:
- High-level file operations
- Directory management
- DAG builder for Merkle trees
- CID caching and optimization

### Primitives (Shared Types)
**Location:** `storage-interfaces/file-system/primitives/`

Common types used across components:
- `DriveInfo`: Drive metadata
- `DirectoryNode`: Protobuf directory structure
- `FileManifest`: File metadata and chunks
- `CommitStrategy`: Checkpoint configuration
- Helper functions for CID computation

## Getting Started

### 1. Choose Your Path

- **Using the system?** → Start with **[User Guide](./USER_GUIDE.md)**
- **Managing the system?** → Start with **[Admin Guide](./ADMIN_GUIDE.md)**
- **Developing with APIs?** → Start with **[API Reference](./API_REFERENCE.md)**
- **Understanding the design?** → Start with **[FILE_SYSTEM_INTERFACE.md](./FILE_SYSTEM_INTERFACE.md)**

### 2. Install and Configure

```bash
# Add dependencies to Cargo.toml
[dependencies]
file-system-client = { path = "storage-interfaces/file-system/client" }
file-system-primitives = { path = "storage-interfaces/file-system/primitives" }
```

### 3. Run Examples

```bash
# See working examples
cargo run --example user_workflow_simplified
cargo run --example admin_workflow_simplified
```

### 4. Test the System

```bash
# Run pallet tests
cargo test -p pallet-drive-registry

# Run client SDK tests
cargo test -p file-system-client

# Run integration tests
just start-services  # Terminal 1
bash scripts/quick-test.sh  # Terminal 2
```

## Examples

Complete examples are available in:
- `storage-interfaces/file-system/examples/user_workflow_simplified.rs`
- `storage-interfaces/file-system/examples/admin_workflow_simplified.rs`
- `storage-interfaces/file-system/examples/basic_usage.rs`

## Testing

```bash
# Test primitives
cargo test -p file-system-primitives

# Test pallet
cargo test -p pallet-drive-registry

# Test client SDK
cargo test -p file-system-client

# Run all Layer 1 tests
cargo test -p file-system-primitives -p pallet-drive-registry -p file-system-client
```

## Future Enhancements

### Planned (Layer 1)
- [ ] Batch operations (multiple files → single commit)
- [ ] Indexer service (off-chain metadata indexing)
- [ ] Search API (full-text search on file names)
- [ ] Path resolution helpers
- [ ] Symbolic links support

### Future (Layer 2)
- [ ] FUSE driver for local mounting
- [ ] Web dashboard (Google Drive-like UI)
- [ ] CLI tools (`fs-cli ls`, `fs-cli cp`, etc.)
- [ ] WebDAV server
- [ ] Access control (W3ACL/UCAN integration)
- [ ] File sharing and permissions

## Related Documentation

### Design Documents
- **[Three-Layered Architecture](../design/scalable-web3-storage.md)** - Overall system design
- **[Layer 0 Implementation](../design/scalable-web3-storage-implementation.md)** - Technical details

### Getting Started
- **[Quick Start Guide](../getting-started/QUICKSTART.md)** - Get running in 5 minutes
- **[Manual Testing Guide](../testing/MANUAL_TESTING_GUIDE.md)** - Testing procedures

### Reference
- **[Extrinsics Reference](../reference/EXTRINSICS_REFERENCE.md)** - Layer 0 blockchain API
- **[Payment Calculator](../reference/PAYMENT_CALCULATOR.md)** - Calculate storage costs

## Support

### Documentation Issues
If you find issues or have suggestions for the documentation:
1. Check existing documentation first
2. Search for related issues
3. Open an issue with:
   - Which document
   - What's unclear/missing
   - Suggested improvement

### Technical Issues
For technical issues:
1. Check logs with `RUST_LOG=debug`
2. Run verification: `bash scripts/verify-setup.sh`
3. Review error codes in API Reference
4. Open an issue with:
   - Error message
   - Steps to reproduce
   - System information

### Community
- **Discord**: [Link to Discord]
- **Forum**: [Link to Forum]
- **GitHub**: [Repository link]

## Contributing

When contributing to File System Interface:
1. Keep Layer 0 dependencies minimal
2. Follow DAG/content-addressed patterns
3. Add comprehensive tests
4. Update documentation
5. Follow Rust/FRAME best practices

See **[CLAUDE.md](../../CLAUDE.md)** for code standards.

## FAQ

**Q: Do I need to understand Layer 0 to use Layer 1?**
A: No! That's the whole point. Layer 1 completely abstracts Layer 0.

**Q: How much does storage cost?**
A: Depends on provider pricing. Use the [Payment Calculator](../reference/PAYMENT_CALCULATOR.md).

**Q: Can I access old versions of my files?**
A: Yes! Each root CID is a snapshot. Save root CIDs to access historical versions.

**Q: What happens if a provider fails?**
A: If you have replicas (3+ providers), other providers take over automatically.

**Q: How do I choose between commit strategies?**
A:
- Immediate: Real-time collaboration
- Batched: Normal usage (default)
- Manual: Batch operations, controlled checkpoints

**Q: Can I change commit strategy after creating a drive?**
A: Not currently. You'd need to create a new drive and migrate data.

**Q: What's the maximum file size?**
A: Limited only by drive capacity. Large files are automatically chunked.

**Q: Are files encrypted?**
A: Not by default. Add client-side encryption if needed.

**Q: Can I share files with other users?**
A: Not yet. File sharing is planned for Layer 2.

## License

Apache 2.0 - See [LICENSE](../../LICENSE) for details.

---

**Need help?** Start with the guide for your role:
- 👤 **Users**: [User Guide](./USER_GUIDE.md)
- 🔧 **Admins**: [Admin Guide](./ADMIN_GUIDE.md)
- 💻 **Developers**: [API Reference](./API_REFERENCE.md)
