# Simplified User Flows: Bucket-Based Model

## Key Insight
**Use existing Layer 0 bucket membership instead of inventing new concepts!**

Layer 0 buckets already have:
- **Admin role**: Manages bucket, agreements, members
- **Reader role**: Can read data
- **Writer role**: Can write data

## Design Principle
**1 Bucket = 1 User's Storage**

- Admin creates bucket and manages infrastructure
- Admin assigns ONE user as (Reader + Writer) to bucket
- User creates drive on their assigned bucket
- User performs file operations

## Admin Flow

### Step 1: Create Bucket (Layer 0)
```rust
let bucket_id = storage_provider.create_bucket(min_providers = 3);
// Admin is automatically the bucket admin
```

### Step 2: Request Storage Agreements (Layer 0)
```rust
// Admin requests agreements with providers
let agreement_1 = storage_provider.request_agreement(
    bucket_id,
    provider_1,
    max_bytes: 100_GB,
    duration: 30_days,
    max_payment: 100_tokens,
    replica_params: None, // Primary
);

let agreement_2 = storage_provider.request_agreement(
    bucket_id,
    provider_2,
    max_bytes: 100_GB,
    duration: 30_days,
    max_payment: 50_tokens,
    replica_params: Some(ReplicaOf(agreement_1)), // Replica
);

// etc for more providers...
```

### Step 3: Assign User to Bucket (Layer 0)
```rust
// Add Alice as Reader+Writer to this bucket
storage_provider.add_bucket_member(
    bucket_id,
    alice_account,
    role: Role::Reader | Role::Writer, // Both roles
);

// Now Alice can use this bucket for her drive!
```

### Step 4: Monitor & Manage (Layer 0)
```rust
// Admin monitors challenges
// Admin replaces failed providers
// Admin adjusts capacity as needed
```

**Admin responsibilities:**
- ✓ Bucket creation
- ✓ Provider agreements
- ✓ Member management
- ✓ Challenge monitoring
- ✓ Provider replacement

**Admin does NOT:**
- ✗ Upload user files
- ✗ Manage user directory structures
- ✗ Commit user changes

---

## User Flow

### Step 1: Discover Available Buckets
```rust
// User queries: "Which buckets can I use?"
let my_buckets = drive_registry.list_user_buckets(alice_account);

// Returns buckets where Alice is Reader+Writer
// [{
//   bucket_id: 42,
//   capacity: 100_GB,
//   available: 100_GB,
//   admin: admin_account,
// }]
```

### Step 2: Create Drive on Bucket (Layer 1)
```rust
// User creates drive on their assigned bucket
let drive_id = drive_registry.create_drive_on_bucket(
    bucket_id: 42,
    root_cid: empty_root_cid,
    name: Some("My Documents"),
);

// System verifies:
// - Bucket exists
// - User is Reader+Writer on bucket
// - Bucket is not already used by another drive
```

### Step 3: File Operations (Client SDK)
```rust
// Upload file
fs_client.upload_file(drive_id, "/report.pdf", data).await?;

// Create folder
fs_client.create_directory(drive_id, "/images").await?;

// List directory
let entries = fs_client.list_directory(drive_id, "/").await?;

// Download file
let data = fs_client.download_file(drive_id, "/report.pdf").await?;
```

**User responsibilities:**
- ✓ File uploads/downloads
- ✓ Folder creation
- ✓ File management

**User does NOT:**
- ✗ Create buckets
- ✗ Manage agreements
- ✗ Handle challenges
- ✗ Replace providers

---

## Complete Example

### Admin: Setup Infrastructure

```rust
use storage_provider_client::StorageProviderClient;
use drive_registry_client::DriveRegistryClient;

#[tokio::main]
async fn main() -> Result<()> {
    let admin_client = StorageProviderClient::new("ws://127.0.0.1:2222")
        .with_signer(admin_keypair);

    // 1. Create bucket
    println!("Creating bucket...");
    let bucket_id = admin_client.create_bucket(min_providers = 3).await?;
    println!("✓ Bucket {} created", bucket_id);

    // 2. Request agreements with 3 providers
    println!("Requesting storage agreements...");

    let agreement_1 = admin_client.request_agreement(
        bucket_id,
        provider_1,
        100 * GB,
        30 * DAYS,
        100 * TOKENS,
        None, // Primary
    ).await?;
    println!("✓ Primary agreement: {}", agreement_1);

    let agreement_2 = admin_client.request_agreement(
        bucket_id,
        provider_2,
        100 * GB,
        30 * DAYS,
        50 * TOKENS,
        Some(ReplicaOf(agreement_1)),
    ).await?;
    println!("✓ Replica 1 agreement: {}", agreement_2);

    let agreement_3 = admin_client.request_agreement(
        bucket_id,
        provider_3,
        100 * GB,
        30 * DAYS,
        50 * TOKENS,
        Some(ReplicaOf(agreement_1)),
    ).await?;
    println!("✓ Replica 2 agreement: {}", agreement_3);

    // 3. Assign user to bucket
    println!("Assigning Alice to bucket...");
    admin_client.add_bucket_member(
        bucket_id,
        alice_account,
        Role::Reader | Role::Writer,
    ).await?;
    println!("✓ Alice can now use bucket {}", bucket_id);

    println!("\n✓ Setup complete! Alice can create her drive now.");
    Ok(())
}
```

### User: Use Storage

```rust
use file_system_client::FileSystemClient;

#[tokio::main]
async fn main() -> Result<()> {
    let fs_client = FileSystemClient::new(
        "ws://127.0.0.1:2222",
        "http://provider.example.com",
    ).with_signer(alice_keypair).await?;

    // 1. Check available buckets
    println!("Checking available storage...");
    let buckets = fs_client.list_my_buckets().await?;

    for bucket in &buckets {
        println!("  Bucket {}: {} GB available",
            bucket.id, bucket.available_gb);
    }

    let bucket_id = buckets[0].id;

    // 2. Create drive
    println!("\nCreating drive on bucket {}...", bucket_id);
    let drive_id = fs_client.create_drive_on_bucket(
        bucket_id,
        Some("My Documents"),
    ).await?;
    println!("✓ Drive {} created", drive_id);

    // 3. Upload files
    println!("\nUploading files...");

    let file1 = std::fs::read("report.pdf")?;
    fs_client.upload_file(drive_id, "/report.pdf", &file1).await?;
    println!("✓ Uploaded report.pdf");

    let file2 = std::fs::read("presentation.pptx")?;
    fs_client.upload_file(drive_id, "/presentation.pptx", &file2).await?;
    println!("✓ Uploaded presentation.pptx");

    // 4. Create folder
    println!("\nCreating folder...");
    fs_client.create_directory(drive_id, "/images").await?;
    println!("✓ Created /images");

    // 5. List directory
    println!("\nListing files:");
    let entries = fs_client.list_directory(drive_id, "/").await?;
    for entry in entries {
        let type_str = if entry.is_directory { "DIR" } else { "FILE" };
        println!("  [{}] {} ({} bytes)", type_str, entry.name, entry.size);
    }

    // 6. Download file
    println!("\nDownloading file...");
    let data = fs_client.download_file(drive_id, "/report.pdf").await?;
    std::fs::write("./downloaded_report.pdf", data)?;
    println!("✓ Downloaded report.pdf");

    println!("\n✓ All operations complete!");
    println!("Note: Alice never touched buckets, agreements, or providers!");

    Ok(())
}
```

---

## Data Model

### Layer 0 (Existing)
```rust
// Bucket (already exists in Layer 0)
struct Bucket {
    id: u64,
    members: Vec<Member>,
    agreements: Vec<AgreementId>,
}

struct Member {
    account: AccountId,
    role: Role, // Admin | Reader | Writer
}
```

### Layer 1 (Simplified)
```rust
// Drive - references bucket, user manages files
pub struct DriveInfo<AccountId, BlockNumber, MaxNameLength> {
    pub owner: AccountId,
    pub bucket_id: u64,          // References Layer 0 bucket
    pub root_cid: Cid,           // Current state
    pub pending_root_cid: Option<Cid>, // Uncommitted changes
    pub commit_strategy: CommitStrategy,
    pub created_at: BlockNumber,
    pub last_committed_at: BlockNumber,
    pub name: Option<BoundedVec<u8, MaxNameLength>>,
}

// NO StoragePool needed!
// NO agreement_ids in DriveInfo!
// Bucket already has all that info!
```

---

## Validation Rules

### When User Creates Drive

```rust
pub fn create_drive_on_bucket(
    origin: OriginFor<T>,
    bucket_id: u64,
    root_cid: Cid,
    name: Option<Vec<u8>>,
) -> DispatchResult {
    let who = ensure_signed(origin)?;

    // 1. Check bucket exists (query Layer 0)
    ensure!(
        pallet_storage_provider::Buckets::contains_key(bucket_id),
        Error::<T>::BucketNotFound
    );

    // 2. Check user is Reader+Writer on bucket
    let bucket = pallet_storage_provider::Buckets::get(bucket_id);
    let user_roles = bucket.get_member_roles(&who);
    ensure!(
        user_roles.contains(Role::Reader) && user_roles.contains(Role::Writer),
        Error::<T>::InsufficientBucketPermissions
    );

    // 3. Check bucket not already used by another drive
    ensure!(
        !BucketToDrive::<T>::contains_key(bucket_id),
        Error::<T>::BucketAlreadyUsed
    );

    // 4. Create drive
    let drive_id = NextDriveId::<T>::get();
    // ... create drive ...

    // 5. Map bucket -> drive
    BucketToDrive::<T>::insert(bucket_id, drive_id);

    Ok(())
}
```

---

## Benefits

### Simplicity
- ✅ Uses existing Layer 0 bucket model
- ✅ No new "storage pool" concept
- ✅ No agreement tracking in DriveInfo
- ✅ Clean separation: Layer 0 = infrastructure, Layer 1 = files

### Flexibility
- ✅ Admin can create buckets for different users
- ✅ Admin can adjust capacity per bucket
- ✅ Admin can set different policies per bucket
- ✅ Users isolated from infrastructure complexity

### Security
- ✅ Leverages existing bucket permissions
- ✅ Role-based access control (Reader, Writer)
- ✅ Admin retains infrastructure control
- ✅ Users can't break infrastructure

---

## Common Scenarios

### Scenario 1: Personal Use
```
Admin (Alice) creates bucket for herself
Alice adds Alice as Reader+Writer
Alice creates drive and uses it
```

### Scenario 2: Organization
```
Admin (IT department) creates buckets
Admin assigns employees as Reader+Writer per bucket
Employees create drives and use storage
IT monitors and maintains infrastructure
```

### Scenario 3: Service Provider
```
Admin (Storage provider company) creates buckets
Admin assigns customers as Reader+Writer
Customers pay monthly, get bucket access
Provider handles all infrastructure
```

---

## Migration from Current Design

### Phase 1: Add new extrinsic
```rust
// New: create_drive_on_bucket (simplified)
// Old: create_drive_with_storage (complex)
// Both work during transition
```

### Phase 2: Deprecate old extrinsic
```rust
#[deprecated(note = "Use create_drive_on_bucket instead")]
pub fn create_drive_with_storage(...) { ... }
```

### Phase 3: Remove old code
```rust
// Remove StoragePool concept
// Remove agreement_ids from DriveInfo
// Clean architecture
```

---

## Comparison: Storage Pools vs Bucket-Based

| Aspect | Storage Pools (Complex) | Bucket-Based (Simple) |
|--------|------------------------|----------------------|
| **New Concepts** | StoragePool, PoolAccess, PoolAccessList | None (uses existing) |
| **On-chain Storage** | Pools + Drives | Just Drives |
| **Permission Model** | Custom per pool | Layer 0 bucket roles |
| **Sharing** | Many users per pool | 1 user per bucket |
| **Infrastructure** | Admin manages pools | Admin manages buckets |
| **User Experience** | Pick from pools | Assigned bucket |
| **Code Complexity** | High | Low |

**Winner:** Bucket-Based Model ✅

---

## Implementation Priority

1. ✅ Update DriveInfo (remove agreement_ids, add bucket validation)
2. ✅ Add `create_drive_on_bucket` extrinsic
3. ✅ Add bucket permission checks
4. ✅ Update FileSystemClient
5. ✅ Create examples (admin + user)
6. ✅ Write tests
7. ✅ Remove StoragePool code (clean up)

This is MUCH simpler! Should we implement this approach instead?
