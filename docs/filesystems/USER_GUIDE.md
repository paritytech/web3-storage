# File System Interface - User Guide

## Table of Contents

1. [Introduction](#introduction)
2. [Getting Started](#getting-started)
3. [Creating Your First Drive](#creating-your-first-drive)
4. [File Operations](#file-operations)
5. [Directory Operations](#directory-operations)
6. [Drive Management](#drive-management)
7. [Advanced Configuration](#advanced-configuration)
8. [How Provider Selection Works](#how-provider-selection-works)
9. [How Checkpoints Work](#how-checkpoints-work)
10. [Best Practices](#best-practices)
11. [Troubleshooting](#troubleshooting)

---

## Introduction

The File System Interface allows you to store and manage files on decentralized storage without worrying about the underlying infrastructure. Think of it as your personal cloud storage, but decentralized, verifiable, and censorship-resistant.

### What You Can Do

- ✅ Create multiple drives with different storage configurations
- ✅ Upload and download files of any size
- ✅ Organize files in directories
- ✅ Access historical versions of your data
- ✅ Customize storage redundancy and commit frequency

### What You Don't Need to Worry About

- ❌ Finding storage providers
- ❌ Managing storage agreements
- ❌ Handling provider failures
- ❌ Distributing payments
- ❌ Creating buckets or managing infrastructure

The system handles all of this automatically!

---

## Getting Started

### Prerequisites

1. **Account with Tokens**: You need a funded account to pay for storage
2. **Client SDK**: Install the File System Client SDK
3. **Running Network**: Access to the parachain RPC endpoint

### Installation

```bash
# Add to your Cargo.toml
[dependencies]
file-system-client = { path = "path/to/storage-interfaces/file-system/client" }
file-system-primitives = { path = "path/to/storage-interfaces/file-system/primitives" }
```

### Initialize Client

```rust
use file_system_client::FileSystemClient;

// Initialize client with blockchain connection
let mut fs_client = FileSystemClient::new(
    "ws://127.0.0.1:2222",           // Parachain WebSocket endpoint
    "http://127.0.0.1:3333",         // Storage provider HTTP endpoint
).await?;

// Set up signing (for testing with dev accounts)
fs_client = fs_client
    .with_dev_signer("alice")        // Use Alice's dev account
    .await?;

// Or use a real keypair for production:
// use subxt_signer::sr25519::Keypair;
// let keypair = Keypair::from_seed("your seed phrase")?;
// fs_client = fs_client.with_signer(keypair).await?;
```

**Blockchain Integration:**
The client uses `subxt` to interact with the parachain:
- Submits drive creation transactions
- Updates root CIDs on-chain
- Queries drive metadata
- Extracts transaction events

---

## Creating Your First Drive

A **drive** is your storage space. You specify what you need (size, duration, budget), and the system sets up everything automatically.

### Basic Drive Creation

```rust
use file_system_primitives::CommitStrategy;

// Create a 10 GB drive for 500 blocks
let drive_id = fs_client.create_drive(
    Some("My Documents"),      // Drive name (optional)
    10_000_000_000,            // 10 GB capacity
    500,                       // 500 blocks duration
    1_000_000_000_000,         // 1 token payment (12 decimals)
    None,                      // Auto-select providers
    None,                      // Use default commit strategy
).await?;

println!("✅ Drive created with ID: {}", drive_id);
```

**What happens automatically:**
1. System creates a bucket in Layer 0
2. Selects 1 provider (short-term storage)
3. Requests storage agreement with provider
4. Sets up empty drive structure
5. Configures batched commits (every 100 blocks)

### Understanding the Parameters

| Parameter | Type | Description | Example |
|-----------|------|-------------|---------|
| `name` | `Option<&str>` | Human-readable drive name | `Some("My Documents")` |
| `max_capacity` | `u64` | Maximum storage in bytes | `10_000_000_000` (10 GB) |
| `storage_period` | `u64` | Duration in blocks | `500` (≈50 minutes at 6s/block) |
| `payment` | `u128` | Total payment (12 decimals) | `1_000_000_000_000` (1 token) |
| `min_providers` | `Option<u8>` | Number of providers | `None` (auto), `Some(3)` |
| `commit_strategy` | `Option<CommitStrategy>` | Checkpoint frequency | `None` (default), `Some(...)` |

### Storage Duration Examples

```rust
// Short-term (1 hour at 6s/block)
let storage_period = 600;

// Medium-term (1 day)
let storage_period = 14_400;

// Long-term (1 week)
let storage_period = 100_800;

// Very long-term (1 month)
let storage_period = 432_000;
```

---

## File Operations

### Upload a File

```rust
// Read file from disk
let file_data = std::fs::read("./documents/report.pdf")?;

// Upload to drive
fs_client.upload_file(
    drive_id,
    "/report.pdf",        // Path in drive
    &file_data,
    bucket_id,            // Associated bucket ID
).await?;

println!("✅ File uploaded: /report.pdf");
```

**What happens:**
1. File is split into chunks (if large)
2. Chunks are uploaded to provider
3. FileManifest is created with chunk references
4. Parent directory is updated
5. Changes are queued for next checkpoint

### Upload to Subdirectory

```rust
// Auto-creates parent directories
fs_client.upload_file(
    drive_id,
    "/documents/work/report.pdf",
    &file_data,
    bucket_id,
).await?;

// Creates: /documents/ and /documents/work/ automatically
```

### Download a File

```rust
let file_data = fs_client.download_file(
    drive_id,
    "/report.pdf",
).await?;

// Save to local disk
std::fs::write("./downloaded_report.pdf", file_data)?;

println!("✅ File downloaded: report.pdf");
```

### Delete a File

```rust
fs_client.delete_file(
    drive_id,
    "/old_document.pdf",
    bucket_id,
).await?;

println!("✅ File deleted: /old_document.pdf");
```

**Note:** Deletion updates the directory structure but doesn't immediately remove the data from storage (chunks remain until garbage collected).

---

## Directory Operations

### Create a Directory

```rust
fs_client.create_directory(
    drive_id,
    "/documents",
    bucket_id,
).await?;

println!("✅ Directory created: /documents");
```

### Create Nested Directories

```rust
// Creates all parent directories automatically
fs_client.create_directory(
    drive_id,
    "/work/projects/2024/q1",
    bucket_id,
).await?;
```

### List Directory Contents

```rust
let entries = fs_client.list_directory(
    drive_id,
    "/documents",
).await?;

println!("Contents of /documents:");
for entry in entries {
    let type_str = if entry.is_directory { "DIR" } else { "FILE" };
    println!("  [{}] {}", type_str, entry.name);

    if !entry.is_directory {
        println!("       Size: {} bytes", entry.size);
    }
}
```

**Example Output:**
```
Contents of /documents:
  [DIR] work/
  [DIR] personal/
  [FILE] report.pdf
       Size: 1048576 bytes
  [FILE] notes.txt
       Size: 2048 bytes
```

### Navigate Directory Tree

```rust
// List root
let root_entries = fs_client.list_directory(drive_id, "/").await?;

// List subdirectory
let work_entries = fs_client.list_directory(drive_id, "/work").await?;

// List deeply nested
let entries = fs_client.list_directory(drive_id, "/work/projects/2024").await?;
```

---

## Drive Management

### List Your Drives

```rust
// Query on-chain to get all your drives
let my_drives = query_user_drives(account_id).await?;

for drive_info in my_drives {
    println!("Drive ID: {}", drive_info.drive_id);
    println!("  Name: {:?}", drive_info.name);
    println!("  Capacity: {} bytes", drive_info.max_capacity);
    println!("  Expires: block {}", drive_info.expires_at);
    println!();
}
```

### Rename a Drive

```rust
// Call on-chain extrinsic
update_drive_name(drive_id, Some("Updated Name")).await?;

println!("✅ Drive renamed");
```

### Clear Drive Contents

Wipe all data from a drive while keeping the drive and storage agreements:

```rust
// Removes all files but keeps the drive structure
clear_drive(drive_id).await?;

println!("✅ Drive cleared - all files removed");
```

**What happens:**
- Root CID reset to zero (empty drive)
- All file data markers cleared
- Drive structure remains intact
- Storage agreements continue (no refunds)
- You can immediately start using the drive again

**Use case:** Start fresh with the same drive, seasonal data cleanup, testing/development resets

### Delete a Drive

Permanently remove a drive, including its bucket and all storage agreements:

```rust
// Complete removal with refund
delete_drive(drive_id).await?;

println!("✅ Drive deleted - bucket removed, funds refunded");
```

**What happens:**
1. All storage agreements are ended
2. Providers are paid for time served
3. You receive a prorated refund for unused time
4. The bucket is removed from Layer 0
5. The drive is removed from your account

**Requirements:**
- You must be the drive owner
- Operation is permanent and cannot be undone

**Use case:** No longer need the drive, reclaim unused storage funds

**Choosing between Clear and Delete:**
- **Clear**: Use when you want to keep the drive and agreements but start with empty storage (faster, no refunds)
- **Delete**: Use when you're done with the drive entirely and want to reclaim funds (permanent, with refunds)

### Check Drive Status

```rust
let drive_info = get_drive_info(drive_id).await?;

println!("Drive Status:");
println!("  Owner: {:?}", drive_info.owner);
println!("  Bucket: {}", drive_info.bucket_id);
println!("  Root CID: 0x{}", hex::encode(drive_info.root_cid));
println!("  Capacity: {} / {} bytes", current_usage, drive_info.max_capacity);
println!("  Expires: block {} (current: {})", drive_info.expires_at, current_block);
```

---

## Advanced Configuration

### High Redundancy Storage

For critical data that needs maximum availability:

```rust
let drive_id = fs_client.create_drive(
    Some("Critical Data"),
    5_000_000_000,        // 5 GB
    2_000,                // 2000 blocks (long-term)
    2_000_000_000_000,    // 2 tokens (more providers = more cost)
    Some(5),              // 5 providers (1 primary + 4 replicas)
    None,                 // Batched commits (efficient)
).await?;
```

**Use case:** Company records, legal documents, irreplaceable data

### Real-Time Collaboration

For shared drives where changes need immediate visibility:

```rust
let drive_id = fs_client.create_drive(
    Some("Team Project"),
    10_000_000_000,       // 10 GB
    1_000,                // 1000 blocks
    3_000_000_000_000,    // 3 tokens (immediate commits = more transactions)
    Some(3),              // 3 providers (standard redundancy)
    Some(CommitStrategy::Immediate), // Every change commits immediately
).await?;
```

**Use case:** Shared documents, real-time collaboration, live data

### Manual Checkpoint Control

For batch operations where you want to control checkpoints:

```rust
let drive_id = fs_client.create_drive(
    Some("Batch Upload"),
    50_000_000_000,       // 50 GB
    500,                  // 500 blocks
    5_000_000_000_000,    // 5 tokens
    Some(3),              // 3 providers
    Some(CommitStrategy::Manual), // User controls commits
).await?;

// Upload many files...
fs_client.upload_file(drive_id, "/file1.dat", &data1, bucket_id).await?;
fs_client.upload_file(drive_id, "/file2.dat", &data2, bucket_id).await?;
fs_client.upload_file(drive_id, "/file3.dat", &data3, bucket_id).await?;
// ... upload 1000 files ...

// Manually commit changes once
commit_drive_changes(drive_id).await?;
```

**Use case:** Data migration, bulk uploads, controlled snapshots

### Custom Batched Commits

Control checkpoint frequency:

```rust
// Commit every 50 blocks (more frequent)
let drive_id = fs_client.create_drive(
    Some("Active Project"),
    10_000_000_000,
    1_000,
    2_000_000_000_000,
    None,
    Some(CommitStrategy::Batched { interval: 50 }),
).await?;

// Commit every 500 blocks (less frequent)
let drive_id = fs_client.create_drive(
    Some("Archive"),
    100_000_000_000,
    10_000,
    10_000_000_000_000,
    None,
    Some(CommitStrategy::Batched { interval: 500 }),
).await?;
```

---

## How Provider Selection Works

When you create a drive, the system automatically selects storage providers based on your requirements. Understanding this process helps you optimize your storage setup.

### Automatic Provider Discovery

The system uses the **marketplace matching algorithm** to find suitable providers:

```rust
// Behind the scenes, create_drive does this:
// 1. Query available providers via runtime API
let requirements = StorageRequirements {
    bytes_needed: max_capacity,
    min_duration: storage_period,
    max_price_per_byte: calculated_max_price,
    primary_only: true,
};

// 2. Get providers sorted by match score (0-100)
let matched_providers = find_matching_providers(requirements, limit);

// 3. Select best matches for your drive
let selected = matched_providers.iter().take(min_providers);
```

### Provider Match Scoring

Providers are scored based on how well they meet your requirements:

| Criterion | Score Impact | Description |
|-----------|--------------|-------------|
| Accepting agreements | Required (score=0 if not) | Provider must be accepting new agreements |
| Available capacity | -50 if insufficient | Provider needs `available >= your max_capacity` |
| Price within budget | -30 if too high | Price must be ≤ your `max_price_per_byte` |
| Duration range | -20 if outside range | Your duration must fit provider's min/max |

**Score Interpretation:**
- 100: Perfect match
- 70-99: Good match with minor issues
- 50-69: Partial match (may have limitations)
- <50: Poor match (not recommended)

### Capacity-Aware Selection

Providers declare their maximum storage capacity:

```rust
// Provider's settings include:
ProviderSettings {
    max_capacity: 1_099_511_627_776,  // 1 TB
    // ...
}

// Available capacity = max_capacity - committed_bytes
// The system only selects providers with enough available capacity
```

**Benefits:**
- No failed agreements due to capacity issues
- Better resource allocation across providers
- Predictable storage availability

### Manual Provider Selection

For advanced use cases, you can specify providers manually:

```rust
// Create drive with specific providers
let drive_id = fs_client.create_drive_with_providers(
    Some("Custom Setup"),
    10_000_000_000,
    500,
    1_000_000_000_000,
    vec![provider_1, provider_2, provider_3],  // Your chosen providers
    None,
).await?;
```

**Use cases:**
- Geographically distributed providers for latency optimization
- Known reliable providers from past experience
- Testing with specific provider configurations

For more details, see [Storage Marketplace Design](../design/marketplace.md).

---

## How Checkpoints Work

Checkpoints ensure your data is permanently committed to the blockchain. Layer 1 handles this automatically, but understanding the process helps you choose the right commit strategy.

### Automatic Checkpoint Management

When you create a drive with `CommitStrategy::Batched`:

```rust
// The client automatically:
// 1. Tracks file changes (uploads, deletes, directory updates)
// 2. Periodically collects commitments from all providers
// 3. Verifies consensus (majority agreement on data state)
// 4. Submits checkpoint to blockchain
// 5. Handles provider failures gracefully
```

### Checkpoint Flow

```
Your File Operation → Layer 1 Client → Provider Storage
        ↓
   Change Queued
        ↓
   Interval Reached (e.g., 100 blocks)
        ↓
   Collect Provider Commitments
        ↓
   Verify Consensus (≥51% agree)
        ↓
   Submit Checkpoint On-Chain
        ↓
   Data Now Permanently Recorded
```

### Commit Strategy Details

| Strategy | How It Works | Best For |
|----------|--------------|----------|
| **Immediate** | Checkpoints after every file operation | Real-time collaboration, critical updates |
| **Batched** | Checkpoints every N blocks | Normal usage, cost-efficient |
| **Manual** | You call `commit_drive_changes()` | Bulk uploads, controlled snapshots |

### Checkpoint Metrics

The system tracks checkpoint health automatically:

```rust
// Access checkpoint metrics (advanced users)
let metrics = fs_client.get_checkpoint_metrics(drive_id).await?;

println!("Total checkpoints: {}", metrics.total_attempts);
println!("Successful: {}", metrics.successful_submissions);
println!("Consensus rate: {}%", metrics.average_consensus_rate);
println!("Provider health:");
for (provider, health) in &metrics.provider_health {
    println!("  {}: {} successes, {} failures",
        provider, health.successes, health.failures);
}
```

### Provider Conflict Detection

If providers disagree on the data state, the system detects and handles it:

```rust
// The CheckpointManager (Layer 0) automatically:
// 1. Detects when providers report different MMR roots
// 2. Identifies conflicting providers
// 3. Logs conflict evidence for potential challenges
// 4. Continues with majority consensus
```

For more details, see [Checkpoint Protocol Design](../design/CHECKPOINT_PROTOCOL.md).

---

## Best Practices

### Storage Planning

1. **Estimate Your Needs**
   ```rust
   // Calculate required capacity
   let total_files_size = 8_500_000_000;  // 8.5 GB
   let buffer = 1.2;                       // 20% buffer for metadata
   let max_capacity = (total_files_size as f64 * buffer) as u64;  // ~10 GB
   ```

2. **Choose Appropriate Duration**
   - Short-term (<1000 blocks): Temporary files, caches
   - Medium-term (1000-10000 blocks): Active projects
   - Long-term (>10000 blocks): Archives, backups

3. **Calculate Payment**
   ```rust
   // Check provider price first
   let price_per_byte = 1_000_000;  // per byte per block
   let payment = price_per_byte * max_capacity * storage_period;
   let payment_with_buffer = (payment as f64 * 1.1) as u128;  // 10% buffer
   ```

### Redundancy Strategy

| Data Type | Recommended Providers | Rationale |
|-----------|----------------------|-----------|
| Temporary files | 1 | Cost-effective, acceptable risk |
| Active documents | 3 | Balanced redundancy |
| Important records | 5 | High availability |
| Critical/Legal | 7+ | Maximum protection |

### Commit Strategy Selection

| Scenario | Strategy | Reason |
|----------|----------|--------|
| Bulk upload | Manual | Control checkpoints, save costs |
| Normal usage | Batched (100 blocks) | Balanced |
| Frequent updates | Batched (50 blocks) | More current |
| Real-time collaboration | Immediate | Always up-to-date |
| Archive | Batched (500+ blocks) | Minimal overhead |

### File Organization

```rust
// Good: Organized structure
/documents/
  /work/
    /projects/
      /project-a/
      /project-b/
  /personal/
/images/
  /2024/
    /january/
    /february/

// Avoid: Flat structure with many files
/file1.pdf
/file2.pdf
/file3.pdf
// ... 1000+ files in root
```

### Version Management

```rust
// Access current version
let current_data = fs_client.download_file(drive_id, "/document.pdf").await?;

// Access historical version (via saved root CID)
let old_root_cid = saved_root_cids[0];  // From previous checkpoint
let old_data = fs_client.download_file_at_version(
    drive_id,
    "/document.pdf",
    old_root_cid,
).await?;
```

---

## Troubleshooting

### Common Issues

#### 1. "NoProvidersAvailable" Error

**Problem:** No storage providers available for your requirements

**Solutions:**
- Wait for providers to register
- Reduce `min_providers` count
- Check provider capacity (they might be full)
- Contact administrator to add providers

#### 2. "InsufficientPayment" Error

**Problem:** Payment doesn't cover storage costs

**Solution:**
```rust
// Calculate proper payment
let provider_price = query_provider_price(provider_id).await?;
let required_payment = provider_price * max_capacity * storage_period;
let safe_payment = (required_payment as f64 * 1.2) as u128;  // 20% buffer
```

#### 3. "DriveNotFound" Error

**Problem:** Trying to access non-existent drive

**Solutions:**
- Verify drive_id is correct
- Check if drive was deleted
- Ensure you're querying the right network

#### 4. "NotDriveOwner" Error

**Problem:** Trying to modify someone else's drive

**Solution:**
- Verify you're using the correct account
- Check drive ownership: `get_drive_info(drive_id).owner`

#### 5. Upload Fails

**Problem:** File upload fails silently

**Checklist:**
```rust
// 1. Verify drive exists
let drive_info = get_drive_info(drive_id).await?;

// 2. Check bucket is valid
let bucket_info = get_bucket_info(drive_info.bucket_id).await?;

// 3. Verify provider is active
let provider_info = get_provider_info(provider_id).await?;

// 4. Check available capacity
let used = calculate_used_capacity(drive_id).await?;
let available = drive_info.max_capacity - used;
ensure!(file_size <= available, "Not enough capacity");
```

#### 6. Download Returns Empty

**Problem:** Downloaded file is empty or corrupted

**Solutions:**
- Verify file exists: `list_directory(drive_id, "/parent")`
- Check CID is correct
- Verify provider is online and responsive
- Try different provider if replicas exist

### Debug Tips

```rust
// Enable verbose logging
env::set_var("RUST_LOG", "file_system_client=debug");
tracing_subscriber::fmt::init();

// Check drive state
let drive_info = get_drive_info(drive_id).await?;
println!("Drive state: {:?}", drive_info);

// List all files
fn list_all_files(fs_client: &FileSystemClient, drive_id: DriveId, path: &str) {
    let entries = fs_client.list_directory(drive_id, path).await?;
    for entry in entries {
        println!("{}{}", path, entry.name);
        if entry.is_directory {
            list_all_files(fs_client, drive_id, &format!("{}{}/", path, entry.name));
        }
    }
}
```

### Getting Help

1. **Check Logs**: Enable debug logging to see detailed operations
2. **Check Chain**: Run `scripts/check-chain.sh` to confirm relay + parachain reachable
3. **Contact Support**: Include drive_id, error message, and transaction hash
4. **Community**: Ask in Discord/Forum with reproducible example

---

## Security Considerations

### Data Privacy

**Important**: Data is stored in plaintext by default. The system provides integrity (content-addressing), not confidentiality.

**What is protected:**
- Data integrity via blake2-256 CIDs
- Data availability via provider replication
- Accountability via blockchain commitments

**What is NOT protected:**
- Data confidentiality (anyone with the CID can read)
- Metadata privacy (directory names, file sizes are visible)

### Client-Side Encryption

For sensitive data, encrypt before uploading:

```rust
use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, NewAead};

// Generate a key (store securely!)
let key = Key::from_slice(b"your-32-byte-key-here-");
let cipher = Aes256Gcm::new(key);

// Encrypt
let nonce = Nonce::from_slice(b"unique-nonce"); // Must be unique per encryption
let ciphertext = cipher.encrypt(nonce, plaintext_data.as_ref())?;

// Upload encrypted
fs_client.upload_file(drive_id, "/secret.enc", &ciphertext, bucket_id).await?;

// Download and decrypt
let encrypted = fs_client.download_file(drive_id, "/secret.enc").await?;
let plaintext = cipher.decrypt(nonce, encrypted.as_ref())?;
```

**Key Management:**
- Store encryption keys separately from data
- Never upload keys to the drive
- Consider using a key management service
- Backup keys securely (lose key = lose data access)

### Content Addressing Security

Each file and directory has a CID (Content Identifier):

```
CID = blake2_256(SCALE_encoded_data)
```

This provides:
- **Tamper Detection**: Any modification changes the CID
- **Integrity Verification**: Download → compute CID → compare
- **Deduplication**: Same content has same CID

See [Architecture Document](./ARCHITECTURE.md#content-addressing--cids) for details.

---

## Next Steps

- **[Admin Guide](./ADMIN_GUIDE.md)** - System administration
- **[API Reference](./API_REFERENCE.md)** - Complete API documentation
- **[Examples](../../storage-interfaces/file-system/examples/)** - Code examples

## Additional Resources

- **[Architecture](./ARCHITECTURE.md)** - System design
- **[Architecture Deep Dive](./ARCHITECTURE.md)** - Encoding, security, blockchain details
- **[Layer 0 Documentation](../design/scalable-web3-storage.md)** - Underlying storage
- **[Layer 1 Quick Start](../getting-started/LAYER1_QUICKSTART.md)** — three-terminal setup + SDK examples
