# Implementation TODO: Admin/User Flows

## ✅ Completed

1. **Architecture Design**
   - Created FLOWS.md documenting admin vs user workflows
   - Defined StoragePool concept
   - Updated DriveInfo to reference pools instead of direct agreements

2. **Primitives Updated**
   - Added `StoragePoolId` type
   - Added `StoragePool` struct with capacity, pricing, agreements
   - Added `PoolAccess` enum (Public, Restricted)
   - Updated `DriveInfo` to use `pool_id` and `quota` instead of direct agreements

3. **Pallet Storage Updated**
   - Added `StoragePools` storage map
   - Added `NextPoolId` counter
   - Added `PoolAccessList` for restricted pools
   - Updated `Drives` storage to use simplified DriveInfo

4. **Events Added**
   - `StoragePoolCreated`, `StoragePoolDeactivated`
   - `PoolCapacityUpdated`
   - `PoolAccessGranted`, `PoolAccessRevoked`
   - `DriveCreatedFromPool`

5. **Errors Added**
   - `PoolNotFound`, `PoolInactive`
   - `InsufficientPoolCapacity`, `PoolAccessDenied`
   - `QuotaExceedsCapacity`, `PoolIdOverflow`

## 🚧 In Progress

### Pallet Extrinsics

#### Admin Extrinsics (Priority: HIGH)

```rust
// 1. Create storage pool
#[pallet::call_index(8)]
pub fn create_storage_pool(
    origin: OriginFor<T>,
    bucket_id: u64,
    agreement_ids: Vec<AgreementId>,
    capacity: u64,
    price_per_gb_month: T::Balance,
    batched_commits: bool,
    batch_interval: u32,
    access: PoolAccess,
    name: Option<Vec<u8>>,
) -> DispatchResult

// 2. Deactivate pool
#[pallet::call_index(9)]
pub fn deactivate_pool(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
) -> DispatchResult

// 3. Reactivate pool
#[pallet::call_index(10)]
pub fn reactivate_pool(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
) -> DispatchResult

// 4. Update pool capacity
#[pallet::call_index(11)]
pub fn update_pool_capacity(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
    new_capacity: u64,
) -> DispatchResult

// 5. Grant pool access (for Restricted pools)
#[pallet::call_index(12)]
pub fn grant_pool_access(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
    user: T::AccountId,
) -> DispatchResult

// 6. Revoke pool access
#[pallet::call_index(13)]
pub fn revoke_pool_access(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
    user: T::AccountId,
) -> DispatchResult

// 7. Replace pool provider (when provider fails)
#[pallet::call_index(14)]
pub fn replace_pool_provider(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
    failed_agreement_id: AgreementId,
    new_agreement_id: AgreementId,
) -> DispatchResult
```

#### User Extrinsics (Priority: HIGH)

```rust
// 1. Create drive from pool (SIMPLIFIED)
#[pallet::call_index(15)]
pub fn create_drive_from_pool(
    origin: OriginFor<T>,
    pool_id: StoragePoolId,
    quota: u64,              // How much storage user wants
    name: Option<Vec<u8>>,
) -> DispatchResult {
    // Check pool exists and is active
    // Check user has access (if restricted)
    // Check pool has capacity
    // Create empty root directory
    // Allocate quota from pool
    // Create drive with pool reference
}

// Note: Other user operations (upload, download, etc.) are handled by client SDK
// They don't need on-chain extrinsics
```

### Runtime Integration (Priority: HIGH)

Update `runtime/src/lib.rs`:

```rust
impl pallet_drive_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type MaxDriveNameLength = ConstU32<128>;
    type MaxDrivesPerUser = ConstU32<100>;
    type MaxAgreements = ConstU32<10>;
    type Balance = Balance; // NEW: Add Balance type
}
```

### Mock Implementation (Priority: MEDIUM)

Update `pallet-registry/src/mock.rs`:
- Add Balance type to mock runtime
- Update test runtime config
- Fix existing tests for new DriveInfo structure

### Tests (Priority: MEDIUM)

Create comprehensive tests:

```rust
// Admin flow tests
#[test]
fn create_storage_pool_works() { ... }

#[test]
fn deactivate_pool_works() { ... }

#[test]
fn grant_access_to_restricted_pool_works() { ... }

#[test]
fn update_pool_capacity_works() { ... }

// User flow tests
#[test]
fn create_drive_from_public_pool_works() { ... }

#[test]
fn create_drive_from_restricted_pool_requires_access() { ... }

#[test]
fn create_drive_fails_when_pool_full() { ... }

#[test]
fn create_drive_fails_when_pool_inactive() { ... }

// Integration tests
#[test]
fn full_admin_user_workflow() {
    // Admin creates pool
    // User creates drive
    // User uploads files (simulated)
    // Check capacity tracking
}
```

### FileSystemClient Updates (Priority: HIGH)

Update `storage-interfaces/file-system/client/src/lib.rs`:

```rust
impl FileSystemClient {
    /// List available storage pools for user
    pub async fn list_available_pools(&self) -> Result<Vec<PoolInfo>> {
        // Query StoragePools storage
        // Filter by access permissions
        // Return pool info with pricing
    }

    /// Create drive from pool (SIMPLIFIED USER FLOW)
    pub async fn create_drive_from_pool(
        &mut self,
        pool_id: StoragePoolId,
        quota: u64,
        name: Option<&str>,
    ) -> Result<DriveId> {
        // 1. Create empty root directory
        let root = DirectoryNode::new_empty("root");
        let root_cid = root.compute_cid()?;
        let root_bytes = root.to_bytes()?;

        // 2. Get pool info to find bucket_id
        let pool = self.query_pool(pool_id).await?;

        // 3. Upload root to pool's bucket
        self.upload_blob(pool.bucket_id, &root_bytes).await?;

        // 4. Call on-chain extrinsic
        let drive_id = self.create_drive_from_pool_on_chain(
            pool_id,
            quota,
            root_cid,
            name,
        ).await?;

        Ok(drive_id)
    }

    // Remove these user-facing methods:
    // - create_drive_with_storage (too complex)
    // - Any bucket/agreement management

    // Keep these:
    // - upload_file
    // - download_file
    // - create_directory
    // - list_directory
}
```

### Examples (Priority: MEDIUM)

Create two new examples:

#### 1. `examples/admin_workflow.rs`

```rust
//! Admin Workflow: Setting up storage infrastructure

async fn main() -> Result<()> {
    // Step 1: Create bucket in Layer 0
    let bucket_id = storage_provider.create_bucket(min_providers = 3);

    // Step 2: Request agreements with providers
    let agreements = vec![
        storage_provider.request_agreement(bucket_id, provider_1, ...),
        storage_provider.request_agreement(bucket_id, provider_2, ...),
        storage_provider.request_agreement(bucket_id, provider_3, ...),
    ];

    // Step 3: Create storage pool
    let pool_id = drive_registry.create_storage_pool(
        bucket_id,
        agreements,
        capacity: 1_TB,
        price: 1_token_per_GB_per_month,
        batched_commits: true,
        batch_interval: 100,
        access: PoolAccess::Public,
        name: "Public Pool",
    );

    println!("Pool {} created! Users can now create drives.", pool_id);
}
```

#### 2. `examples/user_workflow.rs`

```rust
//! User Workflow: Using storage for files

async fn main() -> Result<()> {
    // Step 1: List available pools
    let pools = fs_client.list_available_pools().await?;
    println!("Available pools:");
    for pool in pools {
        println!("  Pool {}: {} GB @ {} tokens/GB",
            pool.id, pool.available_gb, pool.price);
    }

    // Step 2: Create drive from pool
    let drive_id = fs_client.create_drive_from_pool(
        pool_id: 1,
        quota: 10_GB,
        name: "My Documents",
    ).await?;

    // Step 3: Upload files
    fs_client.upload_file(drive_id, "/file1.txt", data1).await?;
    fs_client.upload_file(drive_id, "/file2.txt", data2).await?;

    // Step 4: Create folder
    fs_client.create_directory(drive_id, "/images").await?;

    // Step 5: List directory
    let entries = fs_client.list_directory(drive_id, "/").await?;
    for entry in entries {
        println!("{} ({} bytes)", entry.name, entry.size);
    }

    // Step 6: Download file
    let bytes = fs_client.download_file(drive_id, "/file1.txt").await?;

    println!("User never touched buckets or agreements!");
}
```

## 📋 Future Enhancements

### Phase 2: Auto-Commit Worker (Priority: LOW)
- Off-chain worker that commits pending changes based on strategy
- Watches drives with `Batched` strategy
- Calls `commit_changes()` when interval reached

### Phase 3: Storage Monitor (Priority: LOW)
- Monitor Layer 0 challenges
- Auto-raise disputes
- Notify admins of failures
- Optionally auto-replace providers

### Phase 4: Advanced Features (Priority: LOW)
- Storage tiers (Free, Standard, Premium)
- Shared drives (multi-user collaboration)
- Versioning and time travel
- Auto-expanding quotas
- Provider reputation tracking

## Implementation Order

**Week 1: Core Functionality**
1. ✅ Complete admin extrinsics
2. ✅ Complete user extrinsics
3. ✅ Update runtime config
4. ✅ Fix compilation

**Week 2: Testing & Client**
5. ✅ Update mock runtime
6. ✅ Write comprehensive tests
7. ✅ Update FileSystemClient

**Week 3: Documentation & Examples**
8. ✅ Create admin example
9. ✅ Create user example
10. ✅ Update README with new flows

**Week 4: Integration & Polish**
11. ✅ Integration tests
12. ✅ Performance testing
13. ✅ Security audit
14. ✅ Final documentation

## Notes

- **Breaking Change**: This redesign changes DriveInfo structure
- **Migration**: Existing drives need migration to pools
- **Backwards Compatibility**: Keep old extrinsics deprecated for one release
- **Admin Permissions**: Initially require Sudo, later add governance/staking

## Questions for Review

1. **Admin Permissions**: Should pool creation require:
   - Sudo only?
   - Governance approval?
   - Stake-based (lock X tokens)?

2. **Pricing**: Should pricing be:
   - Fixed (set by admin)?
   - Market-based (providers compete)?
   - Tiered (different rates for different usage)?

3. **Capacity Management**: Should we allow:
   - Over-provisioning (promise more than pool has)?
   - Auto-expansion (automatically increase capacity)?
   - Waitlist (queue users when full)?

4. **Quota Enforcement**: When user exceeds quota:
   - Hard stop (reject uploads)?
   - Grace period (allow temporary excess)?
   - Auto-upgrade (charge more and expand)?
