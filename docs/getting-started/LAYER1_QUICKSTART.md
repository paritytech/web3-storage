# Layer 1 Storage Interfaces Quick Start

This guide helps you get started with the two Layer 1 storage interfaces built on top of Scalable Web3 Storage:

- **File System Interface** - Familiar file/folder operations (create directories, upload files, etc.)
- **S3-Compatible Interface** - Amazon S3-compatible API (buckets, objects, keys)

Both interfaces use the same underlying Layer 0 infrastructure but offer different abstractions.

## Prerequisites

### Required Software

- **Rust** (1.74+): https://rustup.rs/
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup target add wasm32-unknown-unknown
  ```

- **just** (command runner):
  ```bash
  cargo install just
  # or on macOS:
  brew install just
  ```

### One-Time Setup

Run this once to download required binaries and build the project:

```bash
just setup
```

This downloads:
- Polkadot relay chain binaries
- Polkadot omni-node (parachain node)
- Zombienet (local network orchestrator)

And builds:
- Runtime (parachain with storage pallets)
- Provider node (off-chain storage server)

## Starting the Infrastructure

You need two services running: the **blockchain** and the **storage provider**.

### Terminal 1: Start the Blockchain

```bash
just start-chain
```

Wait for output showing:
```
=== Starting Blockchain (Relay Chain + Parachain) ===

Web UIs (once ready):
  Relay chain: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900
  Parachain:   https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222
```

The parachain is ready when you see blocks being produced in the logs.

### Terminal 2: Start the Storage Provider

```bash
just start-provider
```

Wait for output showing:
```
=== Starting Storage Provider Node ===

Provider health: http://127.0.0.1:3333/health
```

### Verify Everything is Running

```bash
# Check provider health
just health

# Expected output:
# {
#   "status": "ok",
#   ...
# }
```

## Network Configuration

Default ports (can be overridden in justfile):

| Service | Port | URL |
|---------|------|-----|
| Relay Chain | 9900 | ws://127.0.0.1:9900 |
| Parachain | 2222 | ws://127.0.0.1:2222 |
| Provider | 3333 | http://127.0.0.1:3333 |

## Using the File System Interface

The File System interface provides familiar file/folder semantics:

### Quick Demo

```bash
# Run the file system example (chain + provider must already be running)
cargo run -p file-system-client --example basic_usage
```

### Rust SDK Usage

```rust
use file_system_client::FileSystemClient;
use file_system_primitives::CommitStrategy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to blockchain and provider
    let mut client = FileSystemClient::new(
        "ws://127.0.0.1:2222",    // Parachain WebSocket
        "http://127.0.0.1:3333",  // Provider HTTP
    ).await?
    .with_dev_signer("alice")     // Use Alice for testing
    .await?;

    // Create a drive (like a mounted filesystem)
    let drive_id = client.create_drive(
        Some("My Drive"),         // Drive name
        10_000_000_000,           // 10 GB capacity
        500,                      // 500 blocks duration
        1_000_000_000_000,        // Payment (1 token, 12 decimals)
        Some(1),                  // 1 provider minimum
        Some(CommitStrategy::Immediate),
    ).await?;
    println!("Created drive: {}", drive_id);

    // Get bucket ID (for file operations)
    let bucket_id = client.get_bucket_id(drive_id).await?;

    // Create directories
    client.create_directory(drive_id, "/documents", bucket_id).await?;
    client.create_directory(drive_id, "/photos", bucket_id).await?;

    // Upload a file
    let content = b"Hello, decentralized storage!";
    client.upload_file(drive_id, "/documents/hello.txt", content, bucket_id).await?;

    // List directory contents
    let entries = client.list_directory(drive_id, "/documents").await?;
    for entry in entries {
        println!("  {} ({} bytes)", entry.name_str(), entry.size);
    }

    // Download and verify
    let downloaded = client.download_file(drive_id, "/documents/hello.txt").await?;
    assert_eq!(downloaded, content);
    println!("Content verified!");

    Ok(())
}
```

### File System Commands

| Command | Description |
|---------|-------------|
| `cargo run -p file-system-client --example basic_usage` | Run basic usage example |
| `just fs-test-all` | Run all file system tests |
| `just fs-demo-ci` | Integration example used by CI (requires chain + provider) |

### File System API Summary

| Operation | Method |
|-----------|--------|
| Create drive | `create_drive(name, capacity, duration, payment, providers, strategy)` |
| Create directory | `create_directory(drive_id, path, bucket_id)` |
| Upload file | `upload_file(drive_id, path, data, bucket_id)` |
| Download file | `download_file(drive_id, path)` |
| List directory | `list_directory(drive_id, path)` |
| Get bucket ID | `get_bucket_id(drive_id)` |

## Using the S3-Compatible Interface

The S3 interface provides Amazon S3-compatible semantics:

### Quick Demo

```bash
# Run the S3 example (chain + provider must already be running)
cargo run -p s3-client --example basic_usage
```

### Rust SDK Usage

```rust
use s3_client::{S3Client, PutObjectOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create S3 client
    let client = S3Client::new(
        "ws://127.0.0.1:2222",    // Parachain WebSocket
        "http://127.0.0.1:3333",  // Provider HTTP
        "//Alice",                 // Seed phrase
    ).await?;

    // Create a bucket
    let bucket = client.create_bucket("my-bucket").await?;
    println!("Created bucket: {:?}", bucket);

    // Upload an object
    let response = client.put_object(
        "my-bucket",
        "folder/hello.txt",
        b"Hello, S3-compatible storage!",
        PutObjectOptions::default(),
    ).await?;
    println!("Uploaded with CID: {:?}", response.cid);

    // List objects
    let objects = client.list_objects_v2("my-bucket", Default::default()).await?;
    for obj in objects.contents {
        println!("  {} ({} bytes)", obj.key, obj.size);
    }

    // Download object
    let data = client.get_object("my-bucket", "folder/hello.txt").await?;
    println!("Downloaded: {}", String::from_utf8_lossy(&data.data));

    // Get object metadata (without downloading)
    let metadata = client.head_object("my-bucket", "folder/hello.txt").await?;
    println!("Content-Type: {:?}", metadata.content_type);

    Ok(())
}
```

### S3 Commands

| Command | Description |
|---------|-------------|
| `cargo run -p s3-client --example basic_usage` | Run basic usage example |
| `just s3-test-all` | Run all S3 tests |
| `just s3-demo-ci` | Integration example used by CI (requires chain + provider) |

### S3 API Summary

#### Bucket Operations

| Operation | Method |
|-----------|--------|
| Create bucket | `create_bucket(name)` |
| Delete bucket | `delete_bucket(name)` |
| List buckets | `list_buckets()` |
| Get bucket info | `head_bucket(name)` |

#### Object Operations

| Operation | Method |
|-----------|--------|
| Upload object | `put_object(bucket, key, data, options)` |
| Download object | `get_object(bucket, key)` |
| Delete object | `delete_object(bucket, key)` |
| Get metadata | `head_object(bucket, key)` |
| Copy object | `copy_object(src_bucket, src_key, dst_bucket, dst_key)` |
| List objects | `list_objects_v2(bucket, params)` |

## File System vs S3: When to Use Which

| Use Case | Recommended Interface |
|----------|----------------------|
| Hierarchical file organization | File System |
| AWS S3 compatibility needed | S3 |
| Simple key-value storage | S3 |
| Complex directory structures | File System |
| Migration from S3 | S3 |
| Desktop-like file management | File System |

Both interfaces:
- Use the same underlying Layer 0 storage
- Benefit from the same security guarantees (checkpoints, challenges, slashing)
- Store data on the same providers

## Understanding the Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Your Application                            │
└───────────────┬─────────────────────────────┬───────────────────┘
                │                             │
                ▼                             ▼
┌───────────────────────────┐   ┌───────────────────────────────┐
│   File System Client      │   │       S3 Client               │
│   - create_drive()        │   │   - create_bucket()           │
│   - upload_file()         │   │   - put_object()              │
│   - list_directory()      │   │   - list_objects_v2()         │
└───────────────┬───────────┘   └───────────────┬───────────────┘
                │                               │
                ▼                               ▼
┌───────────────────────────┐   ┌───────────────────────────────┐
│  pallet-drive-registry    │   │    pallet-s3-registry         │
│  (on-chain drive state)   │   │    (on-chain S3 metadata)     │
└───────────────┬───────────┘   └───────────────┬───────────────┘
                │                               │
                └───────────────┬───────────────┘
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│              pallet-storage-provider (Layer 0)                  │
│   - Bucket management                                           │
│   - Provider registration and stake                             │
│   - Checkpoints and challenges                                  │
│   - Slashing for data loss                                      │
└───────────────────────────────┬─────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                  Storage Provider Node                          │
│   - Actual data storage (off-chain)                             │
│   - HTTP API for uploads/downloads                              │
│   - MMR commitment generation                                   │
└─────────────────────────────────────────────────────────────────┘
```

## Troubleshooting

### "Connection refused" Error

```bash
# Check if services are running
just health

# If not running, start them:
# Terminal 1:
just start-chain
# Terminal 2:
just start-provider
```

### "Bucket not found" Error

Make sure you're using the correct bucket ID. Bucket IDs are auto-incremented starting from 0.

### Ports Already in Use

```bash
# Kill existing processes
pkill -f polkadot
pkill -f storage-provider-node
pkill -f zombienet

# Start fresh
just start-chain   # Terminal 1
just start-provider  # Terminal 2
```

### Build Errors

```bash
# Clean and rebuild
cargo clean
just build
```

### Check Logs

```bash
# Run with debug logging
RUST_LOG=debug cargo run -p file-system-client --example basic_usage
RUST_LOG=debug cargo run -p s3-client --example basic_usage
```

## Web UIs

Once infrastructure is running:

- **Relay Chain Explorer**: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900
- **Parachain Explorer**: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222
- **Provider Health**: http://127.0.0.1:3333/health
- **Provider Stats**: http://127.0.0.1:3333/stats

## Next Steps

### File System
- [User Guide](../filesystems/USER_GUIDE.md)
- [API Reference](../filesystems/API_REFERENCE.md)
- [Architecture](../filesystems/ARCHITECTURE.md)

### S3 Interface
- [S3 README](../../storage-interfaces/s3/README.md)

### Architecture
- [System Design](../design/scalable-web3-storage.md)
- [Checkpoint Protocol](../drafts/CHECKPOINT_PROTOCOL.md)

## Quick Reference Card

```bash
# One-time setup
just setup

# Start services (2 terminals)
just start-chain      # Terminal 1
just start-provider   # Terminal 2

# File System
cargo run -p file-system-client --example basic_usage
just fs-test-all      # Run tests
just fs-demo-ci       # CI integration test

# S3
cargo run -p s3-client --example basic_usage
just s3-test-all      # Run tests
just s3-demo-ci       # CI integration test

# Utilities
just health           # Check provider
just stats            # Provider stats
just build            # Build everything
```
