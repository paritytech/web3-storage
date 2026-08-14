# File System Interface - API Reference

## Table of Contents

1. [Overview](#overview)
2. [On-Chain Extrinsics](#on-chain-extrinsics)
3. [Client SDK](#client-sdk)
4. [Primitives](#primitives)
5. [Storage Queries](#storage-queries)
6. [Events](#events)
7. [Errors](#errors)
8. [Types](#types)

---

## Overview

The File System Interface provides three layers of APIs:

1. **On-Chain Extrinsics**: Blockchain calls for drive registry operations
2. **Client SDK**: High-level Rust library for file system operations
3. **Primitives**: Shared types and utilities

---

## On-Chain Extrinsics

### `create_drive`

Create a new drive with automatic infrastructure setup.

**Signature:**
```rust
pub fn create_drive(
    origin: OriginFor<T>,
    name: Option<Vec<u8>>,
    max_capacity: u64,
    storage_period: BlockNumberFor<T>,
    payment: BalanceOf<T>,
    min_providers: Option<u8>,
    commit_strategy: CommitStrategy,
) -> DispatchResult
```

**Parameters:**
- `origin`: Signed origin (drive creator)
- `name`: Optional human-readable drive name (max 256 bytes)
- `max_capacity`: Maximum storage in bytes
- `storage_period`: Duration in blocks
- `payment`: Total payment for storage (12 decimals)
- `min_providers`: Optional minimum number of providers
  - `None`: Auto-determines based on storage_period
    - ≤1000 blocks: 1 provider
    - >1000 blocks: 3 providers
  - `Some(n)`: Explicitly use n providers
- `commit_strategy`: Checkpoint strategy
  - `CommitStrategy::Immediate`: Commit every change immediately
  - `CommitStrategy::Batched { interval }`: Commit every N blocks
  - `CommitStrategy::Manual`: User manually triggers commits

**Returns:**
- `Ok(())`: Drive created successfully
- Emits: `DriveCreated` event with drive_id

**Automatic Behavior:**
1. Creates bucket in Layer 0
2. Determines provider count (explicit or auto)
3. Selects providers with sufficient capacity
4. Requests storage agreements with providers
5. Distributes payment equally across providers
6. Creates empty drive structure

**Example (via polkadot-js):**
```javascript
api.tx.driveRegistry.createDrive(
  "My Documents",                 // name
  10_000_000_000,                 // 10 GB capacity
  500,                            // 500 blocks
  "1000000000000",                // 1 token payment
  null,                           // auto providers
  { Batched: { interval: 100 } }  // batched every 100 blocks
).signAndSend(account);
```

**Errors:**
- `InvalidStorageSize`: max_capacity is zero
- `InvalidStoragePeriod`: storage_period is zero
- `InvalidPayment`: payment is zero
- `InvalidProviderCount`: min_providers is zero
- `DriveNameTooLong`: name exceeds 256 bytes
- `TooManyDrives`: User has reached max drives limit
- `NoProvidersAvailable`: No providers with sufficient capacity

---

### `update_root_cid`

Update the root CID of a drive after file system changes.

**Signature:**
```rust
pub fn update_root_cid(
    origin: OriginFor<T>,
    drive_id: DriveId,
    new_root_cid: Cid,
) -> DispatchResult
```

**Parameters:**
- `origin`: Signed origin (must be drive owner)
- `drive_id`: Drive identifier
- `new_root_cid`: New root directory CID

**Returns:**
- `Ok(())`: Root CID updated successfully
- Emits: `RootCIDUpdated` event

**Example:**
```javascript
api.tx.driveRegistry.updateRootCid(
  0,                              // drive_id
  "0x1234..."                     // new root CID (32 bytes)
).signAndSend(account);
```

**Errors:**
- `DriveNotFound`: Drive doesn't exist
- `NotDriveOwner`: Caller is not the drive owner

---

### `commit_changes`

Manually commit pending changes (for Manual commit strategy).

**Signature:**
```rust
pub fn commit_changes(
    origin: OriginFor<T>,
    drive_id: DriveId,
) -> DispatchResult
```

**Parameters:**
- `origin`: Signed origin (must be drive owner)
- `drive_id`: Drive identifier

**Returns:**
- `Ok(())`: Changes committed
- Emits: `RootCIDUpdated` event

**Example:**
```javascript
api.tx.driveRegistry.commitChanges(0).signAndSend(account);
```

**Errors:**
- `DriveNotFound`: Drive doesn't exist
- `NotDriveOwner`: Caller is not the drive owner
- `NoPendingChanges`: No changes to commit

---

### `clear_drive`

Clear all data from a drive while keeping the drive structure intact.

**Signature:**
```rust
pub fn clear_drive(
    origin: OriginFor<T>,
    drive_id: DriveId,
) -> DispatchResult
```

**Parameters:**
- `origin`: Signed origin (must be drive owner)
- `drive_id`: Drive identifier

**Returns:**
- `Ok(())`: Drive contents cleared
- Emits: `DriveCleared` event with old root CID

**Behavior:**
1. Resets root_cid to zero (empty drive)
2. Clears any pending_root_cid
3. Keeps drive structure, bucket, and agreements intact
4. No refunds (storage agreements continue)

**Use Case:** Wipe all files but continue using the same drive and storage agreements.

**Example:**
```javascript
api.tx.driveRegistry.clearDrive(0).signAndSend(account);
```

**Errors:**
- `DriveNotFound`: Drive doesn't exist
- `NotDriveOwner`: Caller is not the drive owner

---

### `delete_drive`

Permanently delete a drive, including its bucket and all storage agreements.

**Signature:**
```rust
pub fn delete_drive(
    origin: OriginFor<T>,
    drive_id: DriveId,
) -> DispatchResult
```

**Parameters:**
- `origin`: Signed origin (must be drive owner)
- `drive_id`: Drive identifier

**Returns:**
- `Ok(())`: Drive and bucket deleted successfully
- Emits: `DriveDeleted` event with bucket_id and refunded amount

**Behavior:**
1. Ends all storage agreements with providers
2. Calculates prorated refunds based on remaining time
3. Pays providers for time served
4. Returns unspent funds to owner
5. Removes the bucket from Layer 0
6. Removes the drive from registry

**Use Case:** Completely remove a drive when no longer needed. Owner receives prorated refund for unused storage time.

**Example:**
```javascript
api.tx.driveRegistry.deleteDrive(0).signAndSend(account);
```

**Errors:**
- `DriveNotFound`: Drive doesn't exist
- `NotDriveOwner`: Caller is not the drive owner
- `BucketCleanupFailed`: Failed to cleanup underlying bucket

**Note:** Unlike `clear_drive`, this operation is permanent and cannot be undone.

---

### `update_drive_name`

Update the human-readable name of a drive.

**Signature:**
```rust
pub fn update_drive_name(
    origin: OriginFor<T>,
    drive_id: DriveId,
    name: Option<Vec<u8>>,
) -> DispatchResult
```

**Parameters:**
- `origin`: Signed origin (must be drive owner)
- `drive_id`: Drive identifier
- `name`: New name or None to clear

**Returns:**
- `Ok(())`: Name updated
- Emits: `DriveNameUpdated` event

**Example:**
```javascript
api.tx.driveRegistry.updateDriveName(
  0,
  "Updated Name"
).signAndSend(account);
```

**Errors:**
- `DriveNotFound`: Drive doesn't exist
- `NotDriveOwner`: Caller is not the drive owner
- `DriveNameTooLong`: Name exceeds 256 bytes

---

### Legacy Extrinsics (Deprecated)

#### `create_drive_with_bucket`

**Deprecated:** Use `create_drive()` instead.

Creates a drive using an existing bucket (low-level API).

```rust
#[deprecated = "Use create_drive() instead - it handles bucket creation automatically"]
pub fn create_drive_with_bucket(
    origin: OriginFor<T>,
    bucket_id: u64,
    root_cid: Cid,
    name: Option<Vec<u8>>,
) -> DispatchResult
```

#### `create_drive_on_bucket`

Internal API for bucket-based model (advanced users).

```rust
pub fn create_drive_on_bucket(
    origin: OriginFor<T>,
    bucket_id: u64,
    root_cid: Cid,
    name: Option<Vec<u8>>,
) -> DispatchResult
```

---

## Client SDK

### FileSystemClient

High-level client for file system operations with blockchain integration using `subxt`.

#### Constructor

```rust
pub async fn new(
    chain_endpoint: &str,
    provider_endpoint: &str,
    signer: Signer,
) -> Result<Self>
```

**Parameters:**
- `chain_endpoint`: Parachain WebSocket endpoint (e.g., `"ws://127.0.0.1:2222"`)
- `provider_endpoint`: Storage provider HTTP endpoint (e.g., `"http://127.0.0.1:3333"`)
- `signer`: Signs on-chain extrinsics and provider HTTP requests (the provider
  always enforces auth). Build with `Signer::from_seed("//Alice")` for testing, or
  `Signer::from_seed("<mnemonic>")` / `Signer::from_keypair(...)` for real keys —
  never use dev accounts in production.

**Returns:**
- `Ok(FileSystemClient)`: Client connected to blockchain and provider
- `Err(FsClientError)`: Connection or initialization error

**Example:**
```rust
use file_system_client::{FileSystemClient, Signer};

let mut fs_client = FileSystemClient::new(
    "ws://127.0.0.1:2222",
    "http://127.0.0.1:3333",
    Signer::from_seed("//Alice")?,
).await?;
```

---

### Drive Operations

#### `create_drive`

Create a new drive.

```rust
pub async fn create_drive(
    &mut self,
    name: Option<&str>,
    max_capacity: u64,
    storage_period: u64,
    payment: u128,
    min_providers: Option<u8>,
    commit_strategy: Option<CommitStrategy>,
) -> Result<DriveId>
```

**Parameters:**
- `name`: Optional drive name
- `max_capacity`: Storage size in bytes
- `storage_period`: Duration in blocks
- `payment`: Total payment (12 decimals)
- `min_providers`: Optional provider count
- `commit_strategy`: Optional checkpoint strategy

**Returns:**
- `Ok(DriveId)`: Created drive ID
- `Err(...)`: Error details

**Example:**
```rust
let drive_id = fs_client.create_drive(
    Some("My Documents"),
    10_000_000_000,      // 10 GB
    500,                 // 500 blocks
    1_000_000_000_000,   // 1 token
    None,                // auto providers
    None,                // default strategy
).await?;
```

---

### File Operations

#### `upload_file`

Upload a file to the drive.

```rust
pub async fn upload_file(
    &mut self,
    drive_id: DriveId,
    path: &str,
    data: &[u8],
    bucket_id: u64,
) -> Result<()>
```

**Parameters:**
- `drive_id`: Target drive
- `path`: File path (e.g., `/documents/report.pdf`)
- `data`: File contents
- `bucket_id`: Associated bucket ID

**Returns:**
- `Ok(())`: File uploaded successfully
- `Err(...)`: Error details

**Example:**
```rust
let file_data = std::fs::read("report.pdf")?;

fs_client.upload_file(
    drive_id,
    "/documents/report.pdf",
    &file_data,
    bucket_id,
).await?;
```

**Behavior:**
1. Splits file into chunks (if large)
2. Uploads chunks to provider
3. Creates FileManifest with chunk CIDs
4. Updates parent directory
5. Queues root CID update for next checkpoint

---

#### `download_file`

Download a file from the drive.

```rust
pub async fn download_file(
    &self,
    drive_id: DriveId,
    path: &str,
) -> Result<Vec<u8>>
```

**Parameters:**
- `drive_id`: Source drive
- `path`: File path

**Returns:**
- `Ok(Vec<u8>)`: File contents
- `Err(...)`: Error details

**Example:**
```rust
let data = fs_client.download_file(
    drive_id,
    "/documents/report.pdf",
).await?;

std::fs::write("downloaded_report.pdf", data)?;
```

---

#### `delete_file`

Delete a file from the drive.

```rust
pub async fn delete_file(
    &mut self,
    drive_id: DriveId,
    path: &str,
    bucket_id: u64,
) -> Result<()>
```

**Parameters:**
- `drive_id`: Target drive
- `path`: File path
- `bucket_id`: Associated bucket ID

**Returns:**
- `Ok(())`: File deleted
- `Err(...)`: Error details

**Example:**
```rust
fs_client.delete_file(
    drive_id,
    "/old_document.pdf",
    bucket_id,
).await?;
```

---

### Directory Operations

#### `create_directory`

Create a directory.

```rust
pub async fn create_directory(
    &mut self,
    drive_id: DriveId,
    path: &str,
    bucket_id: u64,
) -> Result<()>
```

**Parameters:**
- `drive_id`: Target drive
- `path`: Directory path
- `bucket_id`: Associated bucket ID

**Returns:**
- `Ok(())`: Directory created
- `Err(...)`: Error details

**Example:**
```rust
fs_client.create_directory(
    drive_id,
    "/documents/work",
    bucket_id,
).await?;
```

**Note:** Creates all parent directories automatically.

---

#### `list_directory`

List directory contents.

```rust
pub async fn list_directory(
    &self,
    drive_id: DriveId,
    path: &str,
) -> Result<Vec<DirectoryEntry>>
```

**Parameters:**
- `drive_id`: Target drive
- `path`: Directory path

**Returns:**
- `Ok(Vec<DirectoryEntry>)`: List of entries
- `Err(...)`: Error details

**Example:**
```rust
let entries = fs_client.list_directory(drive_id, "/documents").await?;

for entry in entries {
    if entry.is_directory {
        println!("[DIR]  {}/", entry.name);
    } else {
        println!("[FILE] {} ({} bytes)", entry.name, entry.size);
    }
}
```

**DirectoryEntry Type:**
```rust
pub struct DirectoryEntry {
    pub name: String,
    pub cid: Cid,
    pub is_directory: bool,
    pub size: u64,        // For files only
    pub modified: u64,    // Block number
}
```

---

### Checkpoint Operations

Layer 1 checkpoint methods delegate to Layer 0's `CheckpointManager` for multi-provider coordination and consensus verification. See [Checkpoint Protocol Design](../drafts/CHECKPOINT_PROTOCOL.md) for details.

**Key Concepts:**
- Layer 1 maps `drive_id` → `bucket_id` automatically
- Layer 0's `CheckpointManager` handles provider communication and consensus
- Checkpoints are submitted on-chain via Layer 0's pallet
- Provider health tracking and conflict detection are handled by Layer 0

#### `submit_checkpoint`

Manually submit a checkpoint for a drive.

```rust
pub async fn submit_checkpoint(
    &self,
    drive_id: DriveId,
    provider_endpoints: Vec<String>,
) -> Result<CheckpointResult>
```

**Parameters:**
- `drive_id`: Drive identifier
- `provider_endpoints`: HTTP endpoints of storage providers

**Returns:**
- `Ok(CheckpointResult)`: Result of checkpoint submission
- `Err(FsClientError)`: Error during submission

**CheckpointResult Variants:**
- `Submitted { block_hash, signers }`: Successfully submitted on-chain
- `InsufficientConsensus { agreeing, required, disagreements }`: Not enough providers agreed
- `ProvidersUnreachable { providers }`: Could not reach providers
- `NoProviders`: No providers configured
- `TransactionFailed { error }`: On-chain transaction failed

**Example:**
```rust
let result = fs_client.submit_checkpoint(
    drive_id,
    vec!["http://127.0.0.1:3333".to_string()],
).await?;

match result {
    CheckpointResult::Submitted { signers, .. } => {
        println!("Checkpoint submitted with {} signers", signers.len());
    }
    CheckpointResult::InsufficientConsensus { agreeing, required, .. } => {
        println!("Only {}/{} providers agreed", agreeing, required);
    }
    _ => { /* handle other cases */ }
}
```

**Use Case:** Manual checkpoint submission for drives with `CommitStrategy::Manual` or when you want explicit control.

---

#### `enable_auto_checkpoints`

Enable automatic batched checkpoints for a drive.

```rust
pub async fn enable_auto_checkpoints(
    &mut self,
    drive_id: DriveId,
    provider_endpoints: Vec<String>,
    interval_blocks: Option<u32>,
    callback: Option<CheckpointCallback>,
) -> Result<()>
```

**Parameters:**
- `drive_id`: Drive identifier
- `provider_endpoints`: HTTP endpoints of storage providers
- `interval_blocks`: Blocks between checkpoints (default: 100)
- `callback`: Optional callback invoked after each checkpoint attempt

**Returns:**
- `Ok(())`: Background loop started
- `Err(FsClientError)`: Failed to start loop

**Behavior:**
1. Starts a background task that monitors for changes
2. File operations automatically mark the drive as "dirty"
3. At each interval, submits checkpoint if changes exist
4. Handles failures with backoff and retry

**Example:**
```rust
use std::sync::Arc;

fs_client.enable_auto_checkpoints(
    drive_id,
    vec!["http://127.0.0.1:3333".to_string()],
    Some(100),  // Every 100 blocks (~10 minutes)
    Some(Arc::new(|bucket_id, result| {
        println!("Checkpoint for bucket {}: {:?}", bucket_id, result);
    })),
).await?;

// File operations now automatically trigger checkpoints
fs_client.upload_file(drive_id, "/file.txt", data, bucket_id).await?;
```

**Use Case:** Set-and-forget checkpoint management for drives with `CommitStrategy::Batched`.

---

#### `disable_auto_checkpoints`

Stop the background checkpoint loop.

```rust
pub async fn disable_auto_checkpoints(&mut self) -> Result<()>
```

**Returns:**
- `Ok(())`: Loop stopped
- `Err(FsClientError)`: Error stopping loop

**Example:**
```rust
fs_client.disable_auto_checkpoints().await?;
```

**Note:** Any pending changes will not be automatically checkpointed after this call. Call `submit_checkpoint()` manually if needed before disabling.

---

#### `request_immediate_checkpoint`

Force immediate checkpoint submission (bypasses batched interval).

```rust
pub async fn request_immediate_checkpoint(&self) -> Result<()>
```

**Returns:**
- `Ok(())`: Immediate checkpoint requested
- `Err(FsClientError)`: Error or loop not running

**Example:**
```rust
// Force checkpoint before a critical operation
fs_client.request_immediate_checkpoint().await?;
```

**Use Case:** Before critical operations when you need guaranteed data durability.

---

#### `is_auto_checkpoints_enabled`

Check if automatic checkpoints are active.

```rust
pub fn is_auto_checkpoints_enabled(&self) -> bool
```

**Returns:**
- `true`: Background loop is running
- `false`: No background loop active

**Example:**
```rust
if fs_client.is_auto_checkpoints_enabled() {
    println!("Auto-checkpoints active");
}
```

---

## Primitives

### DriveInfo

On-chain drive metadata.

```rust
pub struct DriveInfo<
    AccountId: Encode + Decode + MaxEncodedLen,
    BlockNumber: Encode + Decode + MaxEncodedLen,
    MaxNameLength: Get<u32>,
    Balance: Encode + Decode + MaxEncodedLen,
> {
    pub owner: AccountId,
    pub bucket_id: u64,
    pub root_cid: Cid,
    pub pending_root_cid: Option<Cid>,
    pub commit_strategy: CommitStrategy,
    pub created_at: BlockNumber,
    pub last_committed_at: BlockNumber,
    pub name: Option<BoundedVec<u8, MaxNameLength>>,
    pub max_capacity: u64,
    pub storage_period: BlockNumber,
    pub expires_at: BlockNumber,
    pub payment: Balance,
}
```

**Fields:**
- `owner`: Account that created the drive
- `bucket_id`: Associated Layer 0 bucket
- `root_cid`: Current root directory CID
- `pending_root_cid`: Next root CID (for batched commits)
- `commit_strategy`: Checkpoint strategy
- `created_at`: Creation block number
- `last_committed_at`: Last checkpoint block
- `name`: Optional human-readable name
- `max_capacity`: Maximum storage in bytes
- `storage_period`: Duration in blocks
- `expires_at`: Expiration block number
- `payment`: Total payment for storage

---

### CommitStrategy

Checkpoint frequency configuration.

```rust
#[derive(Clone, Copy, Encode, Decode, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
pub enum CommitStrategy {
    Immediate,
    Batched { interval: u32 },
    Manual,
}
```

**Variants:**
- `Immediate`: Commit every change immediately (high cost)
- `Batched { interval }`: Commit every N blocks (balanced)
- `Manual`: User manually triggers commits (low cost)

**Default:**
```rust
impl Default for CommitStrategy {
    fn default() -> Self {
        Self::Batched { interval: 100 }
    }
}
```

---

### DirectoryNode

Protobuf-serialized directory structure.

```protobuf
message DirectoryNode {
  string name = 1;
  repeated DirectoryEntry entries = 2;
  uint64 created = 3;
  uint64 modified = 4;
}

message DirectoryEntry {
  string name = 1;
  bytes cid = 2;
  EntryType type = 3;
  uint64 size = 4;
  uint64 modified = 5;
}

enum EntryType {
  FILE = 0;
  DIRECTORY = 1;
}
```

---

### FileManifest

File metadata and chunk references.

```protobuf
message FileManifest {
  string name = 1;
  uint64 size = 2;
  repeated FileChunk chunks = 3;
  uint64 created = 4;
  uint64 modified = 5;
  string content_type = 6;
}

message FileChunk {
  bytes cid = 1;
  uint64 size = 2;
  uint32 index = 3;
}
```

---

### Cid

Content identifier (blake2-256 hash).

```rust
pub type Cid = H256;  // 32-byte hash

// Compute CID
pub fn compute_cid(data: &[u8]) -> Cid {
    let hash = blake2_256(data);
    H256::from(hash)
}
```

---

## Storage Queries

### Query Drive Info

```rust
// Via RPC
let drive = DriveRegistry::drives(drive_id);

// Via polkadot-js
const drive = await api.query.driveRegistry.drives(driveId);
```

**Returns:** `Option<DriveInfo>`

---

### Query User Drives

```rust
// Via RPC
let drives = DriveRegistry::user_drives(account_id);

// Via polkadot-js
const drives = await api.query.driveRegistry.userDrives(accountId);
```

**Returns:** `Vec<DriveId>`

---

### Query Bucket-to-Drive Mapping

```rust
// Via RPC
let drive_id = DriveRegistry::bucket_to_drive(bucket_id);

// Via polkadot-js
const driveId = await api.query.driveRegistry.bucketToDrive(bucketId);
```

**Returns:** `Option<DriveId>`

---

### Query Next Drive ID

```rust
// Via RPC
let next_id = DriveRegistry::next_drive_id();

// Via polkadot-js
const nextId = await api.query.driveRegistry.nextDriveId();
```

**Returns:** `u64`

---

## Events

### DriveCreated

Emitted when a new drive is created.

```rust
DriveCreated {
    drive_id: DriveId,
    owner: T::AccountId,
    bucket_id: u64,
    root_cid: Cid,
}
```

---

### RootCIDUpdated

Emitted when a drive's root CID is updated (checkpoint).

```rust
RootCIDUpdated {
    drive_id: DriveId,
    old_root_cid: Cid,
    new_root_cid: Cid,
}
```

---

### DriveCleared

Emitted when a drive's contents are cleared.

```rust
DriveCleared {
    drive_id: DriveId,
    owner: T::AccountId,
    old_root_cid: Cid,
}
```

---

### DriveDeleted

Emitted when a drive is permanently deleted.

```rust
DriveDeleted {
    drive_id: DriveId,
    owner: T::AccountId,
    bucket_id: u64,
    refunded: Balance,
}
```

**Fields:**
- `drive_id`: The deleted drive identifier
- `owner`: Account that owned the drive
- `bucket_id`: The Layer 0 bucket that was removed
- `refunded`: Amount of tokens refunded to owner for unused storage time

---

### DriveNameUpdated

Emitted when a drive's name is updated.

```rust
DriveNameUpdated {
    drive_id: DriveId,
    name: Option<Vec<u8>>,
}
```

---

### DriveCreatedOnBucket

Emitted when a drive is created using the bucket-based API.

```rust
DriveCreatedOnBucket {
    drive_id: DriveId,
    owner: T::AccountId,
    bucket_id: u64,
    root_cid: Cid,
}
```

---

## Errors

### InvalidStorageSize

Storage capacity is zero or invalid.

```rust
InvalidStorageSize
```

---

### InvalidStoragePeriod

Storage duration is zero or invalid.

```rust
InvalidStoragePeriod
```

---

### InvalidPayment

Payment amount is zero or insufficient.

```rust
InvalidPayment
```

---

### InvalidProviderCount

Provider count is zero (when explicitly specified).

```rust
InvalidProviderCount
```

---

### DriveNameTooLong

Drive name exceeds 256 bytes.

```rust
DriveNameTooLong
```

---

### DriveNotFound

Specified drive doesn't exist.

```rust
DriveNotFound
```

---

### NotDriveOwner

Caller is not the drive owner.

```rust
NotDriveOwner
```

---

### TooManyDrives

User has reached maximum drives limit.

```rust
TooManyDrives
```

---

### NoProvidersAvailable

No providers available with sufficient capacity.

```rust
NoProvidersAvailable
```

---

### BucketAlreadyUsed

Bucket is already associated with another drive.

```rust
BucketAlreadyUsed
```

---

### BucketCreationFailed

Failed to create bucket in Layer 0.

```rust
BucketCreationFailed
```

---

### BucketCleanupFailed

Failed to cleanup bucket in Layer 0 during drive deletion.

```rust
BucketCleanupFailed
```

**Common Causes:**
- Bucket doesn't exist in Layer 0
- Drive was created using deprecated API without proper Layer 0 integration
- Layer 0 cleanup encountered an error

---

### AgreementRequestFailed

Failed to request storage agreement with provider.

```rust
AgreementRequestFailed
```

---

## Types

### DriveId

```rust
pub type DriveId = u64;
```

Drive identifier (unique, auto-incrementing).

---

### AgreementId

```rust
pub type AgreementId = u64;
```

Storage agreement identifier (from Layer 0).

---

### Cid

```rust
pub type Cid = H256;
```

Content identifier (32-byte blake2-256 hash).

---

### Balance Types

```rust
// In pallet
pub type BalanceOf<T> = <<T as pallet_storage_provider::Config>::Currency
    as Currency<<T as frame_system::Config>::AccountId>>::Balance;

// Typically u128 with 12 decimals
// 1 token = 1_000_000_000_000 (1e12)
```

---

### Block Number Types

```rust
pub type BlockNumberFor<T> = <T as frame_system::Config>::BlockNumber;

// Typically u32 or u64
```

---

## Helper Functions

### Compute CID

```rust
use file_system_primitives::compute_cid;

let data = b"Hello, world!";
let cid = compute_cid(data);
```

---

### Serialize/Deserialize Protobuf

```rust
use file_system_primitives::{DirectoryNode, FileManifest};
use prost::Message;

// Serialize
let node = DirectoryNode { /* ... */ };
let bytes = node.encode_to_vec();

// Deserialize
let node = DirectoryNode::decode(&bytes[..])?;
```

---

## Complete Example

```rust
use file_system_client::{FileSystemClient, Signer};
use file_system_primitives::CommitStrategy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize client
    let mut fs_client = FileSystemClient::new(
        "ws://127.0.0.1:2222",
        "http://127.0.0.1:3333",
        Signer::from_seed("//Alice")?,
    ).await?;

    // 2. Create drive
    let drive_id = fs_client.create_drive(
        Some("My Documents"),
        10_000_000_000,
        500,
        1_000_000_000_000,
        None,
        None,
    ).await?;

    println!("Drive created: {}", drive_id);

    // 3. Upload file
    let data = std::fs::read("report.pdf")?;
    fs_client.upload_file(drive_id, "/report.pdf", &data, bucket_id).await?;
    println!("File uploaded");

    // 4. List directory
    let entries = fs_client.list_directory(drive_id, "/").await?;
    for entry in entries {
        println!("  - {}", entry.name);
    }

    // 5. Download file
    let downloaded = fs_client.download_file(drive_id, "/report.pdf").await?;
    std::fs::write("downloaded.pdf", downloaded)?;
    println!("File downloaded");

    Ok(())
}
```

---

## See Also

- **[User Guide](./USER_GUIDE.md)** - User-friendly documentation
- **[Admin Guide](./ADMIN_GUIDE.md)** - System administration
- **[Architecture](./ARCHITECTURE.md)** - Encoding, security, chain integration
- **[Examples](../../clients/file-system/examples/)** - Code samples
