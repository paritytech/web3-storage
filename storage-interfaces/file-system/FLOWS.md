# User Flows: Admin vs User

This document defines the two distinct user flows for the Layer 1 file system.

## Overview

```
┌────────────────────────────────────────────────────────────────┐
│                      ADMIN FLOW                                │
│  (Infrastructure Management - One-time setup)                  │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. Create storage pool                                        │
│     └─> Define capacity, pricing, providers                    │
│                                                                 │
│  2. Configure policies                                         │
│     └─> Commit strategy, access control                        │
│                                                                 │
│  3. Monitor pool health                                        │
│     └─> Replace failed providers, adjust capacity              │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
                             │
                             │ (Pool available for users)
                             ▼
┌────────────────────────────────────────────────────────────────┐
│                      USER FLOW                                 │
│  (File Operations - Daily usage)                               │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. Create drive from available pool                           │
│     └─> System assigns storage automatically                   │
│                                                                 │
│  2. Upload files                                               │
│     └─> Files stored in pool's bucket                          │
│                                                                 │
│  3. Create folders                                             │
│     └─> Directory structure managed                            │
│                                                                 │
│  4. Read/download files                                        │
│     └─> System retrieves from storage                          │
│                                                                 │
│  User never sees: buckets, agreements, challenges, CIDs        │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

## Admin Flow (Infrastructure Management)

### Responsibilities
- Set up storage infrastructure
- Manage storage providers
- Configure policies and pricing
- Monitor pool health
- Handle failed providers

### Step-by-Step

#### 1. Create Storage Pool

**Prerequisites:**
- Admin account with sudo/governance permissions
- Layer 0 bucket already created
- Storage agreements already established

**Extrinsic:**
```rust
drive_registry.create_storage_pool(
    bucket_id: u64,
    agreement_ids: Vec<AgreementId>,
    capacity: u64,              // e.g., 1 TB = 1_000_000_000_000
    price_per_gb_month: Balance, // e.g., 1 token per GB/month
    default_commit_strategy: CommitStrategy,
    access: PoolAccess,         // Public or Restricted
    name: Option<Vec<u8>>,
)
```

**Example:**
```rust
// Admin creates a 1 TB public storage pool
// Users pay 1 token per GB per month
// Batched commits every 100 blocks (~10 minutes)

drive_registry.create_storage_pool(
    bucket_id: 1,
    agreement_ids: vec![101, 102, 103], // 3 providers (1 primary + 2 replicas)
    capacity: 1_000_000_000_000, // 1 TB
    price_per_gb_month: 1_000_000_000_000, // 1 token (12 decimals)
    default_commit_strategy: CommitStrategy::Batched { interval: 100 },
    access: PoolAccess::Public,
    name: Some(b"Public Storage Pool".to_vec()),
)
```

**What happens:**
1. Pool registered on-chain
2. Capacity tracked (0 used initially)
3. Users can now create drives from this pool
4. Emits `StoragePoolCreated` event

#### 2. Grant Access (for Restricted Pools)

**Extrinsic:**
```rust
drive_registry.grant_pool_access(
    pool_id: StoragePoolId,
    user: AccountId,
)
```

**Example:**
```rust
// Admin grants Alice access to premium pool
drive_registry.grant_pool_access(
    pool_id: 2,
    user: alice_account,
)
```

#### 3. Monitor and Manage

**Query pool status:**
```rust
// Check pool health
let pool = drive_registry.storage_pools(pool_id);
println!("Capacity: {} / {}", pool.used, pool.capacity);
println!("Active: {}", pool.active);
```

**Replace failed provider:**
```rust
// If agreement 102 fails
drive_registry.replace_pool_provider(
    pool_id: 1,
    failed_agreement_id: 102,
    new_agreement_id: 105,
)
```

**Deactivate pool:**
```rust
// Stop new drives from using this pool
drive_registry.deactivate_pool(pool_id: 1)
```

#### 4. Adjust Capacity

**Extrinsic:**
```rust
drive_registry.update_pool_capacity(
    pool_id: StoragePoolId,
    new_capacity: u64,
)
```

**Example:**
```rust
// Increase pool from 1 TB to 2 TB
drive_registry.update_pool_capacity(
    pool_id: 1,
    new_capacity: 2_000_000_000_000,
)
```

---

## User Flow (File Operations)

### Responsibilities
- Create drives
- Manage files and folders
- Read/write data

### Step-by-Step

#### 1. List Available Storage Pools

**Query:**
```rust
// See what storage pools are available
let pools = drive_registry.list_available_pools(user_account);

for pool in pools {
    println!("Pool {}: {} GB available at {} tokens/GB/month",
        pool.id,
        (pool.capacity - pool.used) / 1_000_000_000,
        pool.price_per_gb_month / 1_000_000_000_000
    );
}
```

**Output:**
```
Pool 1: 950 GB available at 1 tokens/GB/month
Pool 2: 500 GB available at 0.5 tokens/GB/month (Premium)
```

#### 2. Create Drive

**Extrinsic:**
```rust
drive_registry.create_drive_from_pool(
    pool_id: StoragePoolId,
    quota: u64,           // Storage quota in bytes
    name: Option<Vec<u8>>,
)
```

**Example:**
```rust
// User creates a 10 GB drive from public pool
fs_client.create_drive_from_pool(
    pool_id: 1,
    quota: 10_000_000_000, // 10 GB
    name: Some(b"My Documents".to_vec()),
).await?
```

**What happens:**
1. System checks pool has capacity
2. System checks user has access
3. Creates empty root directory
4. Uploads root to pool's bucket (via pool's agreements)
5. Allocates quota from pool
6. Returns drive_id
7. User charged: 10 GB × 1 token/GB = 10 tokens/month

**User NEVER needs to know:**
- Bucket ID
- Agreement IDs
- Provider accounts
- Challenge mechanisms

#### 3. Upload File

**Client SDK:**
```rust
// Simple file upload
fs_client.upload_file(
    drive_id,
    "/documents/report.pdf",
    file_bytes,
).await?
```

**What happens under the hood:**
1. Client splits file into chunks
2. Uploads chunks to pool's bucket (transparent)
3. Creates FileManifest
4. Updates directory structure
5. Calculates new root CID
6. Stores as `pending_root_cid` (not yet on-chain)
7. **If pool uses batched commits**: Waits for interval
8. **If pool uses immediate commits**: Commits right away

**Cost:**
- File upload: Layer 0 storage cost (paid to providers)
- Commit (batched): 1 transaction per 100 files
- Commit (immediate): 1 transaction per file

#### 4. Create Folder

**Client SDK:**
```rust
fs_client.create_directory(
    drive_id,
    "/images",
).await?
```

**What happens:**
1. Creates new empty DirectoryNode
2. Uploads to pool's bucket
3. Updates parent directory
4. Updates pending_root_cid

#### 5. List Directory

**Client SDK:**
```rust
let entries = fs_client.list_directory(drive_id, "/documents").await?;

for entry in entries {
    println!("{} ({} bytes)", entry.name, entry.size);
}
```

**Output:**
```
report.pdf (1048576 bytes)
presentation.pptx (2097152 bytes)
notes.txt (4096 bytes)
```

#### 6. Download File

**Client SDK:**
```rust
let file_bytes = fs_client.download_file(
    drive_id,
    "/documents/report.pdf"
).await?;

std::fs::write("./report.pdf", file_bytes)?;
```

**What happens:**
1. Query drive's root CID from chain
2. Traverse DAG to find file
3. Fetch FileManifest
4. Download chunks from pool's bucket
5. Reassemble file

#### 7. Delete File

**Client SDK:**
```rust
fs_client.delete_file(
    drive_id,
    "/documents/old_report.pdf"
).await?
```

**What happens:**
1. Removes entry from parent directory
2. Updates pending_root_cid
3. (Optional) Garbage collect unreferenced chunks

---

## Comparison: Admin vs User

| Aspect | Admin Flow | User Flow |
|--------|-----------|-----------|
| **Frequency** | One-time setup, occasional maintenance | Daily operations |
| **Complexity** | High (infrastructure) | Low (file operations) |
| **Knowledge Required** | Layer 0 concepts (buckets, agreements) | Just files and folders |
| **Extrinsics** | `create_storage_pool`, `replace_pool_provider` | `create_drive_from_pool`, files via SDK |
| **Cost Management** | Set pricing policies | Pay based on usage |
| **Failure Handling** | Replace failed providers | Transparent (handled by admin) |

---

## Example: Complete Workflow

### Admin: Set Up Infrastructure (One-time)

```rust
// 1. Admin creates bucket in Layer 0
let bucket_id = storage_provider.create_bucket(min_providers = 3);

// 2. Admin requests agreements with 3 providers
let agreement_1 = storage_provider.request_agreement(bucket_id, provider_1, ...);
let agreement_2 = storage_provider.request_agreement(bucket_id, provider_2, ...);
let agreement_3 = storage_provider.request_agreement(bucket_id, provider_3, ...);

// 3. Admin creates storage pool
drive_registry.create_storage_pool(
    bucket_id,
    vec![agreement_1, agreement_2, agreement_3],
    capacity: 1_000_000_000_000, // 1 TB
    price_per_gb_month: 1_000_000_000_000, // 1 token/GB/month
    default_commit_strategy: CommitStrategy::Batched { interval: 100 },
    access: PoolAccess::Public,
    name: Some(b"Public Pool".to_vec()),
);

// Done! Users can now use this pool
```

### User: Use Storage (Daily)

```rust
// 1. Create drive
let drive_id = fs_client.create_drive_from_pool(
    pool_id: 1,
    quota: 10_000_000_000, // 10 GB
    name: Some("My Drive"),
).await?;

// 2. Upload files (batched automatically)
fs_client.upload_file(drive_id, "/file1.txt", data1).await?;
fs_client.upload_file(drive_id, "/file2.txt", data2).await?;
fs_client.upload_file(drive_id, "/file3.txt", data3).await?;
// ... 97 more files ...

// 3. After 100 blocks, system commits all 100 files in 1 transaction
// User sees: 100 files uploaded
// Cost: 1 commit transaction (instead of 100)

// 4. Download file
let bytes = fs_client.download_file(drive_id, "/file1.txt").await?;

// User never touched buckets, agreements, or providers!
```

---

## Benefits of This Design

### For Admins
✅ Full control over infrastructure
✅ Can optimize costs and reliability
✅ Can offer different tiers (free, premium, enterprise)
✅ Can monitor and maintain pools independently

### For Users
✅ Simple file operations only
✅ No knowledge of Layer 0 required
✅ Automatic batching = lower costs
✅ Transparent failover (admin handles provider issues)
✅ Familiar interface (like Google Drive, Dropbox)

### For System
✅ Clear separation of concerns
✅ Admin complexity isolated
✅ User experience optimized
✅ Scalable (multiple pools, different policies)

---

## Security Considerations

### Admin Permissions
- Who can create pools?
  - **Option 1**: Sudo/governance only
  - **Option 2**: Any account that locks collateral
  - **Recommendation**: Start with governance, add staking later

### User Quota Enforcement
- Check quota on file upload
- Reject if drive exceeds allocated space
- Emit `QuotaExceeded` event

### Pool Capacity
- Track `used` capacity across all drives
- Prevent over-allocation
- Admin can increase capacity as needed

### Access Control
- Public pools: Anyone can create drives
- Restricted pools: Only approved users
- Can revoke access if needed

---

## Future Enhancements

### 1. Storage Tiers
```rust
enum StorageTier {
    Free,     // 5 GB, basic performance
    Standard, // Pay-per-GB, good performance
    Premium,  // Higher cost, best performance, SLA
}
```

### 2. Shared Drives
```rust
// Multiple users share one drive
fs_client.share_drive(drive_id, collaborator, permissions);
```

### 3. Versioning
```rust
// Time travel - access any historical version
let file = fs_client.download_file_at_block(drive_id, path, block_number);
```

### 4. Quotas with Auto-upgrade
```rust
// Automatically expand quota when close to limit
pool.config.auto_expand = true;
pool.config.max_quota_per_user = 100 GB;
```

---

## Migration Path

### From Current Design
Users who already used `create_drive_with_storage`:
1. Admin creates pools
2. System migrates existing drives to appropriate pools
3. Old extrinsics deprecated but still work (backwards compatibility)

### Gradual Rollout
1. **Phase 1**: Both flows supported
2. **Phase 2**: Encourage pool-based creation
3. **Phase 3**: Deprecate manual bucket management
4. **Phase 4**: Pool-only (clean architecture)
