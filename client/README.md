# Storage Client SDK

Comprehensive off-chain SDK for interacting with the Scalable Web3 Storage system.

## Overview

This SDK provides specialized client types for different user roles in the storage ecosystem:

- **`StorageUserClient`** - For end users storing and retrieving data
- **`ProviderClient`** - For storage providers managing their operations
- **`AdminClient`** - For bucket administrators managing buckets and agreements
- **`ChallengerClient`** - For third parties verifying data integrity
- **`DiscoveryClient`** - For finding and matching providers based on requirements

And advanced management tools:

- **`CheckpointManager`** - Multi-provider checkpoint coordination and consensus
- **`EventSubscriber`** - Real-time blockchain event monitoring
- **`CheckpointPersistence`** - State persistence with backup rotation

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
storage-client = { path = "path/to/client" }
tokio = { version = "1", features = ["full"] }
```

## Quick Start

### Setup

All clients take their signer at construction and must connect to the chain
before on-chain operations:

```rust
use storage_client::{AdminClient, ClientConfig, Signer};

let config = ClientConfig::default(); // ws://localhost:2222
// Dev account for testing — use a real keypair in production!
let mut client = AdminClient::new(config, Signer::from_seed("//Alice")?)?;

// Connect to chain
client.connect().await?;

// Now ready for on-chain operations
```

See [INTEGRATION.md](INTEGRATION.md) for detailed substrate integration guide.

### For Storage Users

Upload, download, and verify data:

```rust
use storage_client::{ChunkingStrategy, ClientConfig, Signer, StorageUserClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client (default config, dev Alice signer)
    let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;

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
        "/ip4/203.0.113.1/tcp/3333".to_string(), // multiaddr
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
use storage_client::{AdminClient, Signer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AdminClient::with_defaults(Signer::from_seed("//Alice")?)?;

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
            ChunkLocation { leaf_index: 5, chunk_index: 123 },
        ).await?;

        println!("Challenge created: {:?}", challenge_id);
    }

    // Aggregate challenge activity (challengers earn no reward — a slash goes
    // entirely to the Treasury and the challenger is only refunded its deposit).
    let stats = client.get_challenge_stats().await?;
    println!(
        "Challenges: total={} successful={} failed={}",
        stats.total_challenges, stats.successful_challenges, stats.failed_challenges
    );

    Ok(())
}
```

### For Provider Discovery

Find providers that match your storage requirements:

```rust
use storage_client::{DiscoveryClient, StorageRequirements};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = DiscoveryClient::with_defaults()?;
    client.connect().await?;

    // Define requirements
    let requirements = StorageRequirements {
        bytes_needed: 10 * 1024 * 1024 * 1024, // 10 GB
        min_duration: 100_000,                  // blocks
        max_price_per_byte: 1_000_000,          // budget
        primary_only: true,
    };

    // Find matching providers (sorted by score 0-100)
    let providers = client.find_providers(requirements.clone(), 10).await?;

    for provider in &providers {
        println!("Provider {}: score={}, available={:?} bytes",
            provider.account,
            provider.match_score,
            provider.available_capacity
        );
    }

    // Or get the best match directly
    if let Some(best) = client.find_best_provider(requirements).await? {
        println!("Best provider: {} (score={})", best.account, best.match_score);
    }

    // Or get recommendations with cost estimates
    let recommendations = client.suggest_providers(
        10 * 1024 * 1024 * 1024, // bytes
        100_000,                  // duration
        1_000_000_000_000,        // budget
    ).await?;

    for rec in recommendations {
        println!("{}: {} (cost estimate: {})",
            rec.provider.account,
            rec.reason,
            rec.estimated_cost
        );
    }

    Ok(())
}
```

## Architecture

### Client Configuration

All clients can be configured with custom settings:

```rust
use storage_client::{ClientConfig, Signer};

let config = ClientConfig {
    chain_ws_url: "ws://localhost:2222".to_string(),
    provider_urls: vec!["http://localhost:3333".to_string()],
    timeout_secs: 30,
    enable_retries: true,
};

let client = StorageUserClient::new(config, Signer::from_seed("//Alice")?)?;
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

### DiscoveryClient

- ✅ Find providers matching storage requirements
- ✅ Capacity-aware provider search
- ✅ Match scoring (0-100 based on requirements fit)
- ✅ Provider recommendations with cost estimates
- ✅ Paginated provider listing

### CheckpointManager

- ✅ Multi-provider checkpoint coordination
- ✅ Consensus verification (configurable threshold)
- ✅ Conflict detection and resolution
- ✅ Automatic background checkpointing
- ✅ Provider health tracking and metrics
- ✅ Auto-challenge recommendations

### EventSubscriber

- ✅ Real-time blockchain event streaming
- ✅ Event filtering (by bucket, provider, type)
- ✅ Checkpoint and challenge event monitoring
- ✅ Callback-based subscription
- ✅ Automatic reconnection

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
let mut client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;

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

### Checkpoint Management

Coordinate checkpoints across multiple providers with consensus verification:

```rust
use storage_client::{
    CheckpointManager, CheckpointConfig, BatchedCheckpointConfig,
    BatchedInterval, CheckpointResult,
};

// Create checkpoint manager
let manager = CheckpointManager::new(
    "ws://127.0.0.1:2222",
    CheckpointConfig::default()
).await?;

// Add provider endpoints
let manager = manager.with_providers(vec![
    "http://provider1:3000".to_string(),
    "http://provider2:3000".to_string(),
]);

// Manual checkpoint submission
let result = manager.submit_checkpoint(bucket_id).await;
match result {
    CheckpointResult::Success { mmr_root, providers_agreed } => {
        println!("Checkpoint submitted: {} ({} providers agreed)",
            mmr_root, providers_agreed);
    }
    CheckpointResult::InsufficientConsensus { agreed, total } => {
        println!("Failed: only {}/{} providers agreed", agreed, total);
    }
    CheckpointResult::Conflict { conflicts } => {
        println!("Conflict detected! {} providers disagree", conflicts.len());
    }
    _ => {}
}

// Enable automatic checkpoints
let config = BatchedCheckpointConfig {
    interval: BatchedInterval::Blocks(100), // Every 100 blocks
    retry_on_failure: true,
    max_retries: 3,
    ..Default::default()
};

let handle = manager.start_checkpoint_loop(
    bucket_id,
    config,
    |result| println!("Checkpoint result: {:?}", result),
).await?;

// Control the background loop
handle.mark_dirty(bucket_id);     // Signal data changed
handle.submit_now().await?;       // Force immediate checkpoint
handle.stop().await?;             // Stop the loop
```

### Checkpoint State Persistence

Persist checkpoint state across restarts:

```rust
use storage_client::{CheckpointPersistence, PersistenceConfig, StateBuilder};

// Configure persistence
let config = PersistenceConfig {
    state_file: PathBuf::from("/var/lib/storage/checkpoint_state.json"),
    backup_count: 3,
    auto_save: true,
    auto_save_interval: Duration::from_secs(60),
};

let persistence = CheckpointPersistence::new(config)?;

// Load existing state or create new
let state = persistence.load_or_create()?;

// Build state programmatically
let state = StateBuilder::new()
    .with_bucket(1, BucketStatus::default())
    .with_metrics(CheckpointMetrics::default())
    .build();

// Save state (creates backup of previous)
persistence.save(&state)?;
```

### Real-Time Event Subscription

Monitor blockchain events in real-time:

```rust
use storage_client::{
    EventSubscriber, EventFilter, StorageEvent,
    subscribe_checkpoints, subscribe_challenges,
};

// Create subscriber
let subscriber = EventSubscriber::new("ws://127.0.0.1:2222").await?;

// Subscribe to bucket events
let filter = EventFilter::bucket(bucket_id);
let mut stream = subscriber.subscribe(filter).await?;

while let Some(event) = stream.next().await {
    match event {
        StorageEvent::BucketCheckpointed { bucket_id, mmr_root, block } => {
            println!("Bucket {} checkpointed at block {}", bucket_id, block);
        }
        StorageEvent::ChallengeCreated { challenge_id, provider, .. } => {
            println!("Challenge {} against {}", challenge_id, provider);
        }
        StorageEvent::ProviderSlashed { provider, amount, .. } => {
            println!("Provider {} slashed {} tokens", provider, amount);
        }
        _ => {}
    }
}

// Or use convenience functions
let mut checkpoint_stream = subscribe_checkpoints("ws://127.0.0.1:2222", bucket_id).await?;
let mut challenge_stream = subscribe_challenges("ws://127.0.0.1:2222", bucket_id).await?;

// Subscribe with callback
subscribe_with_callback("ws://127.0.0.1:2222", filter, |event| {
    println!("Event: {:?}", event);
}).await?;
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
- Five specialized client types (user, provider, admin, challenger, discovery)
- Core extrinsic submission (register, agreements, challenges)
- Off-chain provider communication (HTTP)
- Client-side verification and monitoring
- Comprehensive error handling
- Provider discovery and matching with scoring
- Provider capacity declaration and enforcement
- Multi-provider checkpoint coordination
- Checkpoint state persistence with backups
- Real-time event subscription and filtering
- Provider health tracking and metrics

### 🚧 In Progress

- Runtime API call integration for discovery
- Geographic provider matching (multiaddr parsing)

### 📋 Planned

- Automatic retry and failover
- Batch operations for efficiency
- Streaming upload/download
- Content-defined chunking
- Local caching
- Reputation-based provider scoring

## License

[Apache-2.0](../LICENSE-APACHE2)

## Contributing

Contributions welcome! Please see the main repository [README](../README.md) for guidelines.
