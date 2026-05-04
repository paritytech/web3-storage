# File System Interface (Layer 1)

Layer 1 file system built on top of Layer 0 (`pallet-storage-provider`). Users
work with drives, directories, and files; the layer hides buckets, storage
agreements, providers, and challenges.

For the architecture deep-dive (encoding, content addressing, security,
commit-strategy tradeoffs, on-chain footprint, "1 bucket = 1 drive" rationale),
see [`docs/filesystems/ARCHITECTURE.md`](../../docs/filesystems/ARCHITECTURE.md).
For the user-facing SDK guide, see
[`docs/filesystems/USER_GUIDE.md`](../../docs/filesystems/USER_GUIDE.md).

## Quick example

```rust
let drive_id = fs_client.create_drive(
    Some("My Documents"),
    10_000_000_000,    // 10 GB
    500,               // 500 blocks
    1_000_000_000_000, // 1 token (12 decimals)
    None,              // auto-pick providers
    None,              // default commit strategy (Batched, every 100 blocks)
).await?;

fs_client.upload_file(drive_id, "/report.pdf", data).await?;
let entries = fs_client.list_directory(drive_id, "/").await?;
let data = fs_client.download_file(drive_id, "/report.pdf").await?;
```

`create_drive` allocates the underlying Layer-0 bucket, requests storage
agreements with providers, and stamps the drive's initial empty root CID
on-chain. `min_providers` defaults to 3 for periods >1000 blocks and 1
otherwise. Examples in `examples/`.

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

## Design notes specific to this pallet

**Names in parent.** Entry names are stored in the parent `DirectoryNode`, not
in the child. Renaming touches only the parent blob; the child's CID stays
stable, keeping caches valid and limiting the rewrite cascade to the path from
the changed directory up to the root.

**Multi-drive per account.** `UserDrives: AccountId → Vec<DriveId>` lets one
account own many drives — useful for splitting personal/work data, public/
private content, or different replication policies, without per-drive ACL
machinery.

**Immutable versioning.** Each on-chain root CID is a complete snapshot, so any
historical state is reachable as long as the underlying chunks persist in the
provider's bucket. Roll-back is just a `update_root_cid` to an older value.

For the rest of the architecture (encoding, content addressing, security,
commit-strategy tradeoffs, on-chain footprint, "1 bucket = 1 drive"), see
[`docs/filesystems/ARCHITECTURE.md`](../../docs/filesystems/ARCHITECTURE.md).

## Testing

```bash
just fs-test-all   # primitives + pallet-registry + client (unit tests)
```

## References

- [Layer 1 Architecture](../../docs/filesystems/ARCHITECTURE.md)
- [Layer 0 Design](../../docs/design/scalable-web3-storage.md)
- [Layer 0 Implementation](../../docs/design/scalable-web3-storage-implementation.md)
- [Protobuf schemas](./primitives/proto/filesystem.proto)
