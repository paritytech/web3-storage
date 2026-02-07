# File System Client SDK

High-level SDK for interacting with the Layer 1 File System Interface built on Scalable Web3 Storage.

## Overview

The File System Client provides a familiar file system abstraction over Layer 0's raw blob storage, allowing you to work with drives, directories, and files without managing the underlying decentralized infrastructure.

**Key Features:**
- **Familiar API** - Work with drives, folders, and files like a traditional file system
- **Automatic Setup** - Drive creation handles bucket creation, provider selection, and agreement setup
- **Blockchain Integration** - Real on-chain integration using `subxt` for trustless storage
- **Content-Addressed** - All data is immutable and verifiable with CIDs
- **Flexible Commits** - Choose when changes are committed (immediate, batched, or manual)
- **Built on Layer 0** - Leverages Scalable Web3 Storage's provider network and game-theoretic guarantees

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
file-system-client = { path = "path/to/storage-interfaces/file-system/client" }
file-system-primitives = { path = "path/to/storage-interfaces/file-system/primitives" }
tokio = { version = "1", features = ["full"] }
```

## Quick Start

### Prerequisites

Before using the client, you need:

1. **Running blockchain node**:
   ```bash
   just start-chain
   # Parachain WebSocket: ws://127.0.0.1:9944
   ```

2. **Running provider node**:
   ```bash
   cargo run --release -p storage-provider-node
   # Provider HTTP: http://localhost:3000
   ```

3. **On-chain setup** (provider registration, etc.):
   ```bash
   bash scripts/verify-setup.sh
   ```

### Basic Usage

```rust
use file_system_client::FileSystemClient;
use file_system_primitives::CommitStrategy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Connect to blockchain and provider
    let mut fs_client = FileSystemClient::new(
        "ws://127.0.0.1:9944",      // Parachain endpoint
        "http://localhost:3000"      // Provider endpoint
    )
    .await?
    .with_dev_signer("alice")       // Use Alice's key for testing
    .await?;

    // 2. Create a drive (10 GB, 500 blocks duration)
    let drive_id = fs_client
        .create_drive(
            Some("My Documents"),              // Drive name
            10_000_000_000,                     // 10 GB capacity
            500,                                 // 500 blocks duration
            1_000_000_000_000,                  // Payment (1 token with 12 decimals)
            None,                                // Auto-select providers
            Some(CommitStrategy::Batched { interval: 100 }), // Commit every 100 blocks
        )
        .await?;

    println!("✅ Drive created: {}", drive_id);

    // Note: You'll need the bucket_id from the drive info
    let bucket_id = 1u64; // Query this from on-chain state

    // 3. Create directories
    fs_client.create_directory(drive_id, "/documents", bucket_id).await?;
    fs_client.create_directory(drive_id, "/documents/work", bucket_id).await?;

    // 4. Upload a file
    let content = b"Hello, decentralized world!";
    fs_client
        .upload_file(drive_id, "/documents/hello.txt", content, bucket_id)
        .await?;

    println!("✅ File uploaded: /documents/hello.txt");

    // 5. List directory contents
    let entries = fs_client.list_directory(drive_id, "/documents").await?;
    for entry in entries {
        let icon = if entry.is_directory() { "📁" } else { "📄" };
        println!("{} {} ({} bytes)", icon, entry.name, entry.size);
    }

    // 6. Download and verify
    let downloaded = fs_client
        .download_file(drive_id, "/documents/hello.txt")
        .await?;

    assert_eq!(downloaded, content);
    println!("✅ File verified!");

    Ok(())
}
```

## Blockchain Integration

### Connecting to the Chain

The client uses `subxt` for blockchain interaction:

```rust
// Connect to parachain
let fs_client = FileSystemClient::new(
    "ws://127.0.0.1:9944",      // Your parachain WebSocket
    "http://localhost:3000"      // Your provider HTTP endpoint
)
.await?;
```

### Setting Up a Signer

For development, use dev accounts:

```rust
// Use a development account
let fs_client = fs_client
    .with_dev_signer("alice")   // alice, bob, charlie, dave, eve, ferdie
    .await?;
```

For production, use real keypairs:

```rust
use subxt_signer::sr25519::Keypair;

// Load from seed phrase or file
let keypair = Keypair::from_seed("your seed phrase here")?;

let fs_client = fs_client
    .with_signer(keypair)
    .await?;
```

### On-Chain Operations

The client performs these on-chain operations automatically:

- **`create_drive()`** - Submits `DriveRegistry::create_drive` extrinsic
- **`update_root_cid()`** - Updates drive root after file operations (based on commit strategy)
- **`clear_drive()`** - Clears all drive contents
- **`delete_drive()`** - Deletes the drive

## API Overview

### Drive Management

```rust
// Create a drive
let drive_id = fs_client.create_drive(
    Some("Drive Name"),
    capacity_bytes,
    duration_blocks,
    payment,
    min_providers,
    commit_strategy,
).await?;

// List your drives
let drives = fs_client.list_drives().await?;

// Get drive info
let info = fs_client.get_drive_info(drive_id).await?;

// Clear drive contents
fs_client.clear_drive(drive_id).await?;

// Delete drive
fs_client.delete_drive(drive_id).await?;
```

### Directory Operations

```rust
// Create directory
fs_client.create_directory(drive_id, "/path/to/dir", bucket_id).await?;

// List directory contents
let entries = fs_client.list_directory(drive_id, "/path").await?;
for entry in entries {
    println!("{}: {} bytes", entry.name, entry.size);
}
```

### File Operations

```rust
// Upload file
let data = b"File contents";
fs_client.upload_file(drive_id, "/path/to/file.txt", data, bucket_id).await?;

// Download file
let data = fs_client.download_file(drive_id, "/path/to/file.txt").await?;

// Delete file
fs_client.delete_file(drive_id, "/path/to/file.txt", bucket_id).await?;
```

## Commit Strategies

Control when changes are committed to the blockchain:

### Immediate
Every operation commits immediately. Best for real-time collaboration.

```rust
CommitStrategy::Immediate
```

### Batched (Default)
Commits every N blocks. Balanced approach for most use cases.

```rust
CommitStrategy::Batched { interval: 100 }  // Every 100 blocks
```

### Manual
User controls when to commit. Most efficient for batch operations.

```rust
CommitStrategy::Manual

// Later, manually commit:
fs_client.commit_changes(drive_id).await?;
```

## Examples

See the [`examples/`](examples/) directory for complete workflows:

### Basic Usage Example

Demonstrates the complete file system workflow:

```bash
# Prerequisites
just start-chain                              # Terminal 1
cargo run --release -p storage-provider-node  # Terminal 2
bash scripts/verify-setup.sh                  # Verify setup

# Run example
cargo run --example basic_usage
```

The example shows:
1. Connecting to blockchain and provider
2. Creating a drive with proper parameters
3. Building directory structure
4. Uploading files to different paths
5. Listing directory contents
6. Downloading and verifying files

## Architecture

### Layer 1 Components

```
┌─────────────────────────────────────────┐
│  FileSystemClient (This Package)        │
│  - High-level file operations           │
│  - Directory management                 │
│  - Blockchain integration (subxt)       │
└─────────────────────────────────────────┘
                   ▲
                   │
┌─────────────────────────────────────────┐
│  SubstrateClient                        │
│  - Chain connection                     │
│  - Transaction submission               │
│  - Event extraction                     │
│  - Storage queries                      │
└─────────────────────────────────────────┘
                   ▲
                   │
┌─────────────────────────────────────────┐
│  DriveRegistry Pallet (On-Chain)        │
│  - Drive metadata                       │
│  - Root CID tracking                    │
│  - Bucket mapping                       │
└─────────────────────────────────────────┘
```

### Integration with Layer 0

The file system client uses Layer 0's StorageClient:

```
FileSystemClient
    ├── SubstrateClient (on-chain: drives, root CIDs)
    └── StorageClient (off-chain: blobs, chunks)
```

Operations flow:
1. **Upload**: File → Chunks → StorageClient → Provider
2. **Build DAG**: Compute CIDs, build directory tree
3. **Commit**: Update root CID via SubstrateClient
4. **Download**: Query root → Traverse DAG → Fetch chunks

## Error Handling

All operations return `Result<T, FsClientError>`:

```rust
use file_system_client::FsClientError;

match fs_client.upload_file(drive_id, "/test.txt", data, bucket_id).await {
    Ok(_) => println!("Upload successful"),
    Err(FsClientError::DriveNotFound(id)) => eprintln!("Drive {} not found", id),
    Err(FsClientError::StorageClient(msg)) => eprintln!("Storage error: {}", msg),
    Err(FsClientError::Blockchain(msg)) => eprintln!("Blockchain error: {}", msg),
    Err(e) => eprintln!("Error: {}", e),
}
```

## Testing

```bash
# Run unit tests
cargo test -p file-system-client

# Run with logging
RUST_LOG=debug cargo test -p file-system-client

# Run specific test
cargo test -p file-system-client test_create_directory
```

## Status

### ✅ Implemented

- Full file system operations (create, read, list, delete)
- Directory hierarchy management
- Real blockchain integration with subxt
- Drive lifecycle management
- Flexible commit strategies
- Content-addressed storage with CIDs
- Integration with Layer 0 StorageClient

### 🚧 In Progress

- Batch operations (multiple files in one commit)
- Directory deletion (recursive)
- File metadata queries

### 📋 Planned

- Symbolic links
- File permissions and ACLs
- Search and indexing
- Path resolution helpers
- Streaming upload/download

## Comparison with Layer 0

| Feature | Layer 0 (StorageClient) | Layer 1 (FileSystemClient) |
|---------|------------------------|----------------------------|
| **Abstraction** | Raw blob storage | File system (drives/folders/files) |
| **Setup** | Manual (10+ steps) | Automatic (1-2 steps) |
| **Data Organization** | Flat (buckets) | Hierarchical (directories) |
| **User Audience** | Developers | End users + Developers |
| **Complexity** | High | Low |
| **Use Case** | Custom storage logic | General-purpose file storage |

**When to use Layer 0:** Building custom storage applications, need full control

**When to use Layer 1:** General file storage, familiar file system interface

## Documentation

For more details, see:

- **[User Guide](../../../docs/filesystems/USER_GUIDE.md)** - Complete user workflows
- **[Admin Guide](../../../docs/filesystems/ADMIN_GUIDE.md)** - System administration
- **[API Reference](../../../docs/filesystems/API_REFERENCE.md)** - Complete API docs
- **[File System Interface](../../../docs/filesystems/FILE_SYSTEM_INTERFACE.md)** - Architecture and design

## License

Apache-2.0

## Contributing

Contributions welcome! Please:
1. Follow Rust/FRAME best practices
2. Add tests for new features
3. Update documentation
4. Keep Layer 0 dependencies minimal

See [CLAUDE.md](../../../CLAUDE.md) for code standards.
