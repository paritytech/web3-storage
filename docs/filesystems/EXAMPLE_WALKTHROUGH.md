# File System Client - Example Walkthrough

This guide walks you through the `basic_usage.rs` example, demonstrating the complete Layer 1 file system workflow with real blockchain integration.

## Overview

The example demonstrates:
1. **Blockchain Connection** - Connect using `subxt` with proper signer setup
2. **Drive Creation** - Create a drive with automatic infrastructure setup
3. **Directory Structure** - Build nested directories
4. **File Uploads** - Upload files to different paths
5. **Directory Listing** - Navigate and list directory contents
6. **File Downloads** - Download and verify file integrity

**Location:** `storage-interfaces/file-system/client/examples/basic_usage.rs`

## Prerequisites

Before running the example, you need a running infrastructure:

### 1. Start the Blockchain

```bash
# Terminal 1 - Start relay chain + parachain
just start-chain

# Wait for:
# ✓ Relay chain: ws://127.0.0.1:9900
# ✓ Parachain: ws://127.0.0.1:9944
```

**Verify blockchain is running:**
```bash
bash scripts/check-chain.sh
# Should show: "Parachain is ready"
```

### 2. Start Provider Node

```bash
# Terminal 2 - Start storage provider
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
export CHAIN_RPC=ws://127.0.0.1:9944
cargo run --release -p storage-provider-node

# Wait for:
# Storage provider listening on http://0.0.0.0:3000
```

**Verify provider is running:**
```bash
curl http://localhost:3000/health
# Should return: {"status":"ok"}
```

### 3. Complete On-Chain Setup

```bash
# Terminal 3 - Verify on-chain state
bash scripts/verify-setup.sh

# This checks:
# ✓ Provider is registered
# ✓ Provider settings are configured
# ✓ System is ready for storage
```

If setup is incomplete, follow the [Quick Start Guide](../getting-started/QUICKSTART.md).

## Running the Example

Once infrastructure is ready:

```bash
cd storage-interfaces/file-system/client
cargo run --example basic_usage
```

**Expected output:**
```
🚀 File System Client - Basic Usage Example

============================================================

📡 Step 1: Connecting to blockchain and provider...
✅ Connected successfully!

📁 Step 2: Creating a new drive...
✅ Drive created with ID: 0
   Name: My Documents
   Capacity: 10 GB
   Duration: 500 blocks

📂 Step 3: Creating directory structure...
   Creating /documents...
   ✅ Created /documents
   Creating /documents/work...
   ✅ Created /documents/work
   Creating /photos...
   ✅ Created /photos

📝 Step 4: Uploading files...
   Uploading /README.md (92 bytes)...
   ✅ Uploaded /README.md
   Uploading /documents/work/report.txt (75 bytes)...
   ✅ Uploaded /documents/work/report.txt
   Uploading /documents/notes.txt (93 bytes)...
   ✅ Uploaded /documents/notes.txt

📋 Step 5: Listing directory contents...

   Contents of /:
   📁 documents (0 bytes)
   📁 photos (0 bytes)
   📄 README.md (92 bytes)

   Contents of /documents:
   📁 work (0 bytes)
   📄 notes.txt (93 bytes)

   Contents of /documents/work:
   📄 report.txt (75 bytes)

⬇️  Step 6: Downloading and verifying files...

   Downloading /README.md...
   ✅ Downloaded 92 bytes
   ✅ Content verified!
   Content preview: # My Documents

Welcome to my decentralized fil

   Downloading /documents/work/report.txt...
   ✅ Downloaded 75 bytes
   ✅ Content verified!
   Content:
   Q4 2024 Report

   Revenue: $1M
   Growth: 50%

============================================================

🎉 Example completed successfully!

📊 Summary:
   ✅ Created drive: 0
   ✅ Created 3 directories
   ✅ Uploaded 3 files
   ✅ Listed directory contents
   ✅ Downloaded and verified files

💡 Next steps:
   - Try clearing the drive: clear_drive()
   - Try deleting the drive: delete_drive()
   - Explore more file operations
   - Check the on-chain state via polkadot.js
```

## Step-by-Step Breakdown

### Step 1: Client Connection

```rust
let mut fs_client = FileSystemClient::new(
    "ws://127.0.0.1:9944",      // Parachain WebSocket endpoint
    "http://localhost:3000"      // Provider HTTP endpoint
)
.await?
.with_dev_signer("alice")       // Use Alice for testing
.await?;
```

**What happens:**
1. **Connects to blockchain** - Establishes WebSocket connection to parachain at port 9944
2. **Connects to provider** - Sets HTTP endpoint for off-chain storage operations
3. **Sets signer** - Configures Alice's dev account for transaction signing

**Blockchain Integration:**
- Uses `subxt::OnlineClient` to connect to the parachain
- Loads runtime metadata dynamically
- Prepares for transaction submission

### Step 2: Drive Creation

```rust
let drive_id = fs_client
    .create_drive(
        Some("My Documents"),              // Drive name
        10_000_000_000,                     // 10 GB capacity
        500,                                 // 500 blocks duration
        1_000_000_000_000,                  // 1 token payment (12 decimals)
        None,                                // Auto-determine providers
        Some(CommitStrategy::Batched { interval: 100 }), // Batch commits every 100 blocks
    )
    .await?;
```

**What happens:**
1. **Constructs extrinsic** - Builds `DriveRegistry::create_drive` transaction with dynamic encoding
2. **Signs transaction** - Signs with Alice's keypair
3. **Submits to chain** - Sends transaction to parachain
4. **Watches events** - Waits for `Finalized` status
5. **Extracts drive_id** - Parses `DriveCreated` event to get new drive ID

**On-Chain Effects:**
- Creates entry in `Drives` storage map
- Adds drive to Alice's `UserDrives` list
- Returns drive ID (e.g., `0`)

**Automatic Setup (Future):**
- Bucket creation in Layer 0 (planned)
- Provider selection and agreement setup (planned)

**Parameters Explained:**
- `10_000_000_000` bytes = 10 GB
- `500` blocks ≈ 50 minutes (6s per block)
- `1_000_000_000_000` = 1 token with 12 decimals
- `None` = Auto-select provider count based on duration
- `Batched { interval: 100 }` = Commit every 100 blocks (≈10 minutes)

### Step 3: Directory Creation

```rust
fs_client.create_directory(drive_id, "/documents", bucket_id).await?;
fs_client.create_directory(drive_id, "/documents/work", bucket_id).await?;
fs_client.create_directory(drive_id, "/photos", bucket_id).await?;
```

**What happens for each directory:**
1. **Query current root** - Gets drive's current root CID from on-chain storage
2. **Build directory node** - Creates `DirectoryNode` protobuf structure
3. **Upload directory data** - Uploads directory node to provider via HTTP
4. **Compute CID** - Calculates blake2-256 hash of directory data
5. **Update parent** - Recursively updates parent directories to include new entry
6. **Commit (if needed)** - Updates root CID on-chain based on commit strategy

**Directory Structure:**
```
Root (/)
├── documents/     → CID: 0xabc...
│   ├── work/      → CID: 0xdef...
│   └── notes.txt  → CID: 0x123...
├── photos/        → CID: 0x456...
└── README.md      → CID: 0x789...
```

**Content-Addressed:**
Each directory is stored as a blob with its own CID. The root CID represents the entire file system state.

### Step 4: File Upload

```rust
let readme_content = b"# My Documents\n\nWelcome to my decentralized file system!";
fs_client.upload_file(drive_id, "/README.md", readme_content, bucket_id).await?;
```

**What happens:**
1. **Parse path** - Splits `/README.md` into directory (`/`) and filename (`README.md`)
2. **Chunk file** - Splits content into chunks (if large)
3. **Upload chunks** - Uploads each chunk to provider via HTTP POST
4. **Build manifest** - Creates `FileManifest` with chunk CIDs
5. **Upload manifest** - Stores manifest as a blob
6. **Update directory** - Adds file entry to parent directory
7. **Update ancestors** - Recursively updates parent directories up to root
8. **Commit (if needed)** - Updates root CID on-chain

**For small files (<1MB):** Single chunk is used

**For large files:** Multiple chunks with manifest tracking all CIDs

**Nested paths (`/documents/work/report.txt`):**
1. Traverses down to `/documents/work/`
2. Adds `report.txt` entry
3. Updates `/documents/work/` directory
4. Updates `/documents/` directory
5. Updates `/` root directory

### Step 5: Directory Listing

```rust
let entries = fs_client.list_directory(drive_id, "/documents").await?;
for entry in entries {
    let entry_type = if entry.is_directory() { "📁" } else { "📄" };
    println!("{} {} ({} bytes)", entry_type, entry.name, entry.size);
}
```

**What happens:**
1. **Query root CID** - Gets drive's current root from blockchain
2. **Traverse path** - Follows path components to target directory:
   - Start at root `/`
   - Look up `documents` entry
   - Get its CID
3. **Download directory** - Fetches directory node from provider
4. **Parse entries** - Decodes protobuf `DirectoryNode`
5. **Return list** - Returns all entries with names, types, sizes, CIDs

**Entry types:**
- **Directory** - Has `directory_node` field set (CID to nested directory)
- **File** - Has `file` field set (CID to file manifest)

**Sizes:**
- Directories: 0 bytes (metadata only)
- Files: Sum of all chunk sizes

### Step 6: File Download

```rust
let downloaded = fs_client.download_file(drive_id, "/documents/work/report.txt").await?;
```

**What happens:**
1. **Query root CID** - Gets current root from blockchain
2. **Traverse path** - Navigates to `/documents/work/report.txt`
3. **Get file manifest** - Downloads manifest blob from provider
4. **Parse manifest** - Extracts chunk CIDs
5. **Download chunks** - Fetches each chunk from provider via HTTP GET
6. **Reconstruct file** - Concatenates chunks in sequence order
7. **Verify integrity** - Each chunk's CID is verified against expected hash

**Verification:**
- Each chunk is content-addressed (CID = blake2-256 hash)
- Tampering is immediately detected
- Provider cannot serve incorrect data without detection

## Understanding Blockchain Integration

### Transaction Flow

```
FileSystemClient
    └── create_drive()
        └── SubstrateClient::create_drive_on_chain()
            ├── Build dynamic extrinsic
            ├── Sign with keypair
            ├── Submit via RPC
            ├── Watch for Finalized event
            └── Extract drive_id from events
```

### On-Chain Storage Queries

```rust
// Query drive root CID
let storage_client = self.substrate_client.api().storage().at_latest().await?;

// Manual storage key construction
let pallet_hash = twox_128(b"DriveRegistry");
let storage_hash = twox_128(b"Drives");
let key_hash = blake2_128(&drive_id.to_le_bytes());

// Fetch from chain
let bytes = storage_client.fetch_raw(storage_key).await?;

// Decode DriveInfo
let drive_info = decode_drive_info(bytes)?;
let root_cid = drive_info.root_cid;
```

### Root CID Updates

When commit strategy triggers:

```
FileSystemClient::upload_file()
    └── update_drive_root_cid()
        └── SubstrateClient::update_root_cid()
            ├── Build update_root_cid extrinsic
            ├── Sign and submit
            └── Wait for finalization
```

**Commit strategies:**
- **Immediate**: Every file operation updates on-chain (expensive, real-time)
- **Batched**: Updates every N blocks (balanced, efficient)
- **Manual**: User controls when to commit (most efficient)

## Troubleshooting

### "Connection failed" Error

**Problem:** Cannot connect to blockchain

**Solutions:**
```bash
# Check blockchain is running
bash scripts/check-chain.sh

# Check port is accessible
curl http://localhost:9944
# Should NOT refuse connection

# Restart blockchain
pkill -f polkadot
pkill -f "polkadot-parachain\|polkadot-omni-node"
just start-chain
```

### "Provider unavailable" Error

**Problem:** Cannot reach storage provider

**Solutions:**
```bash
# Check provider is running
curl http://localhost:3000/health

# Check provider logs
tail -f provider-node.log

# Restart provider
cargo run --release -p storage-provider-node
```

### "Drive not found" Error

**Problem:** Drive was not created successfully

**Solutions:**
```bash
# Check on-chain state
bash scripts/verify-setup.sh

# View drive via polkadot.js
open "https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944#/chainstate"
# Query: driveRegistry > drives(0)
```

### "Event not found" Error

**Problem:** Transaction succeeded but event extraction failed

**Debugging:**
```rust
// Enable debug logging
RUST_LOG=debug cargo run --example basic_usage

// Look for:
// - "Submitting create_drive transaction"
// - "Transaction finalized"
// - "Event found: DriveCreated"
```

### "No signer configured" Error

**Problem:** Forgot to call `with_dev_signer()` or `with_signer()`

**Solution:**
```rust
// After creating client, must set signer
let fs_client = FileSystemClient::new(...).await?
    .with_dev_signer("alice").await?;  // Add this!
```

## Next Steps

After running the example successfully:

### 1. Explore Additional Operations

```rust
// Clear all drive contents
fs_client.clear_drive(drive_id).await?;

// Delete entire drive
fs_client.delete_drive(drive_id).await?;

// List all your drives
let drives = fs_client.list_drives().await?;
```

### 2. Modify the Example

Try changing:
- **Drive parameters**: Increase capacity, duration, or provider count
- **Commit strategy**: Try `Immediate` or `Manual`
- **File sizes**: Upload larger files to test chunking
- **Directory depth**: Create deeper nested structures

### 3. Inspect On-Chain State

**Via polkadot.js:**
```
https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944#/chainstate

Queries to try:
- driveRegistry.drives(0) - View drive metadata
- driveRegistry.userDrives(ALICE_ACCOUNT) - List Alice's drives
- driveRegistry.nextDriveId() - See next available ID
```

**Via CLI:**
```bash
# Watch blockchain events
polkadot-js-api --ws ws://127.0.0.1:9944 query.system.events

# Query specific drive
polkadot-js-api --ws ws://127.0.0.1:9944 query.driveRegistry.drives 0
```

### 4. Build Your Own Application

Use the example as a template for your own file storage application:

```rust
// Your app
use file_system_client::FileSystemClient;
use file_system_primitives::CommitStrategy;

async fn my_app() -> Result<(), Box<dyn std::error::Error>> {
    let mut fs_client = FileSystemClient::new(
        "ws://your-parachain:9944",
        "http://your-provider:3000",
    )
    .await?
    .with_signer(your_keypair)
    .await?;

    // Build your application logic here
    // ...

    Ok(())
}
```

## Related Documentation

- **[User Guide](USER_GUIDE.md)** - Complete user workflows
- **[API Reference](API_REFERENCE.md)** - Full API documentation
- **[Client README](../../storage-interfaces/file-system/client/README.md)** - SDK documentation
- **[Quick Start](../getting-started/QUICKSTART.md)** - Initial setup

## Summary

The `basic_usage.rs` example demonstrates the complete Layer 1 file system workflow:

✅ **Blockchain Integration** - Real on-chain transactions via `subxt`
✅ **Drive Management** - Create drives with automatic setup
✅ **File Operations** - Upload, download, verify files
✅ **Directory Management** - Hierarchical organization
✅ **Content-Addressed Storage** - All data is verifiable via CIDs
✅ **Flexible Commits** - Control when changes go on-chain

This provides a solid foundation for building decentralized file storage applications on Scalable Web3 Storage!
