# Storage Client SDK

Comprehensive off-chain SDK for interacting with the Scalable Web3 Storage system.

## Overview

This SDK provides specialized client types for different user roles in the storage ecosystem:

- **`StorageUserClient`** - For end users storing and retrieving data
- **`ProviderClient`** - For storage providers managing their operations
- **`AdminClient`** - For bucket administrators managing buckets and agreements
- **`ChallengerClient`** - For third parties verifying data integrity

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
storage-client = { path = "path/to/client" }
tokio = { version = "1", features = ["full"] }
```

## Quick Start

### Setup

All clients that need on-chain access must connect to the chain and set a signer:

```rust
use storage_client::{AdminClient, ClientConfig};

let config = ClientConfig::default(); // ws://localhost:9944
let mut client = AdminClient::new(config, "5GrwvaEF...".to_string())?;

// Connect to chain
client.base.connect_chain().await?;

// Set signer (for testing - use proper keypairs in production!)
client.base = client.base.with_dev_signer("alice")?;

// Now ready for on-chain operations
```

See [INTEGRATION.md](INTEGRATION.md) for detailed substrate integration guide.

### For Storage Users

Upload, download, and verify data:

```rust
use storage_client::{StorageUserClient, ChunkingStrategy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client
    let mut client = StorageUserClient::with_defaults()?;

    // Connect to chain for commit operations
    client.base.connect_chain().await?;
    client.base = client.base.with_dev_signer("alice")?;

    // Upload data
    let data = b"My important data";
    let data_root = client.upload(
        1,                            // bucket_id
        data,
        ChunkingStrategy::default(),
    ).await?;

    println!("Uploaded with root: 0x{}", hex::encode(data_root.as_bytes()));

    // Commit to chain (makes it official)
    let commitment = client.commit(1, vec![data_root]).await?;
    println!("MMR root: {}", commitment.mmr_root);

    // Download and verify
    let retrieved = client.download(&data_root, 0, data.len() as u64).await?;
    assert_eq!(retrieved, data);

    Ok(())
}
```

### For Storage Providers

Register and manage provider operations:

```rust
use storage_client::ProviderClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ProviderClient::with_defaults("5GrwvaEF...".to_string())?;

    // Register as provider
    client.register(
        "/ip4/203.0.113.1/tcp/3000".to_string(), // multiaddr
        vec![0u8; 32],                           // public key
        10_000_000_000_000,                      // stake
    ).await?;

    // Accept storage agreements
    client.accept_agreement(1).await?;

    // Monitor your stats
    let stats = client.get_stats().await?;
    println!("Reputation: {}/100", stats.reputation);

    Ok(())
}
```

### For Bucket Administrators

Create and manage buckets:

```rust
use storage_client::AdminClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AdminClient::with_defaults("5GrwvaEF...".to_string())?;

    // Create bucket
    let bucket_id = client.create_bucket(2).await?; // min 2 providers

    // Request storage from provider
    client.request_agreement(
        bucket_id,
        "5FHneW46...".to_string(), // provider
        10 * 1024 * 1024 * 1024,    // 10 GB
        100_000,                    // duration (blocks)
        5_000_000_000_000,          // payment
        None,                       // primary (not replica)
    ).await?;

    // Freeze bucket for permanent archival
    client.freeze_bucket(bucket_id, 0).await?;

    Ok(())
}
```

### For Challengers

Monitor and challenge providers:

```rust
use storage_client::ChallengerClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ChallengerClient::with_defaults("5DAAnrj7...".to_string())?;

    // Analyze provider
    let analysis = client.analyze_provider(
        1,                          // bucket_id
        "5FHneW46...".to_string(),  // provider
    ).await?;

    println!("Provider reputation: {}", analysis.reputation);

    // Challenge if suspicious
    if analysis.reputation < 70 {
        let challenge_id = client.challenge_checkpoint(
            1,                          // bucket_id
            "5FHneW46...".to_string(),  // provider
            5,                          // leaf_index
            123,                        // chunk_index
        ).await?;

        println!("Challenge created: {:?}", challenge_id);
    }

    // Check earnings
    let earnings = client.get_total_challenge_earnings().await?;
    println!("Total challenge earnings: {} tokens", earnings);

    Ok(())
}
```

## Architecture

### Client Configuration

All clients can be configured with custom settings:

```rust
use storage_client::ClientConfig;

let config = ClientConfig {
    chain_ws_url: "ws://localhost:9944".to_string(),
    provider_urls: vec!["http://localhost:3000".to_string()],
    timeout_secs: 30,
    enable_retries: true,
};

let client = StorageUserClient::new(config)?;
```

### Error Handling

All client operations return `ClientResult<T>`:

```rust
use storage_client::ClientError;

match client.upload(1, data, Default::default()).await {
    Ok(data_root) => println!("Success: 0x{}", hex::encode(data_root.as_bytes())),
    Err(ClientError::ProviderUnavailable(msg)) => eprintln!("Provider issue: {}", msg),
    Err(ClientError::VerificationFailed) => eprintln!("Data integrity check failed!"),
    Err(e) => eprintln!("Error: {}", e),
}
```

## Features

### StorageUserClient

- ✅ Upload data with chunking and Merkle tree building
- ✅ Download data with integrity verification
- ✅ Commit data roots to on-chain MMR
- ✅ Spot-check providers for data availability
- ✅ Monitor provider performance
- ✅ Replicated uploads to multiple providers

### ProviderClient

- ✅ Register as storage provider with stake
- ✅ Update provider settings (pricing, capacity)
- ✅ Accept storage agreements
- ✅ Respond to challenges with proofs
- ✅ Confirm replica syncs for payment
- ✅ Monitor earnings and reputation

### AdminClient

- ✅ Create and configure buckets
- ✅ Manage bucket members and permissions
- ✅ Request storage agreements
- ✅ Extend or terminate agreements
- ✅ Freeze buckets for permanent archival
- ✅ Delete old data to reduce costs

### ChallengerClient

- ✅ Three challenge modes (checkpoint, offchain, replica)
- ✅ Provider analysis and recommendations
- ✅ Automated challenge strategies
- ✅ Earnings tracking and analytics
- ✅ Find profitable challenge targets

## Examples

See the [`examples/`](examples/) directory for complete workflows:

- `complete_workflow.rs` - End-to-end demonstration of all client types

Run examples with:

```bash
cargo run --example complete_workflow
```

## Advanced Usage

### Automated Spot-Checking

```rust
let mut client = StorageUserClient::with_defaults()?;

// Perform 10 random spot-checks
let (passed, failed) = client.spot_check_batch(
    &data_root,
    10,    // number of checks
    100,   // total chunks
).await?;

println!("Spot-checks: {} passed, {} failed", passed, failed);
```

### Challenger Automation

```rust
let client = ChallengerClient::with_defaults("5DAAnrj7...".to_string())?;

// Automated challenge loop
loop {
    let challenges = client.auto_challenge_strategy(
        70,  // min reputation threshold
        5,   // max challenges per round
    ).await?;

    println!("Created {} challenges", challenges.len());

    tokio::time::sleep(Duration::from_secs(300)).await; // 5 minutes
}
```

### Provider Capacity Management

```rust
let client = ProviderClient::with_defaults("5FHneW46...".to_string())?;

let capacity = client.get_capacity_info().await?;
let utilization = (capacity.committed_bytes as f64 /
                   capacity.available_bytes as f64) * 100.0;

if utilization > 80.0 {
    println!("Warning: {}% capacity used", utilization);
    // Add more stake or reduce commitments
}
```

## Layer 1 File System Interface

For most users, consider using the **Layer 1 File System Client** instead, which provides a familiar file system abstraction (drives, folders, files) over Layer 0's raw blob storage.

**When to use Layer 1 (File System Client):**
- You need a familiar file/folder interface
- You want automatic setup and provider selection
- You're building a general-purpose file storage application
- You prefer simplicity over low-level control

**When to use Layer 0 (Storage Client - this SDK):**
- You need full control over storage operations
- You're building custom storage logic
- You want to implement your own data structures on top of blob storage
- You need direct access to buckets and agreements

**Layer 1 Documentation:** See [File System Interface Docs](../docs/filesystems/README.md)

**Layer 1 Client:** `storage-interfaces/file-system/client/`

## Status

This SDK is under active development.

### ✅ Implemented

- Substrate API integration with subxt
- Four specialized client types (user, provider, admin, challenger)
- Core extrinsic submission (register, agreements, challenges)
- Off-chain provider communication (HTTP)
- Client-side verification and monitoring
- Comprehensive error handling

### 🚧 In Progress

- Event parsing for extracting IDs from transaction results
- Storage queries for reading on-chain state
- Runtime API call integration

### 📋 Planned

- Multi-provider selection strategies
- Automatic retry and failover
- Batch operations for efficiency
- Streaming upload/download
- Content-defined chunking
- Local caching

## License

Apache-2.0

## Contributing

Contributions welcome! Please see the main repository README for guidelines.
