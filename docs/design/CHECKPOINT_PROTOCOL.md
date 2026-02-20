# Automated Checkpoint Protocol

## Overview

This document defines the protocol for automated checkpoint management in Layer 1 (File System Interface). The goal is to completely abstract away the complexity of multi-provider signature collection from end users.

## Problem Statement

Currently, to submit a checkpoint, users must:
1. Know all provider endpoints for their bucket
2. Query each provider for their commitment
3. Verify all providers agree on the same MMR root
4. Collect all signatures
5. Submit the checkpoint transaction on-chain
6. Handle disagreements, retries, and failures

**This is too complex for end users.**

## Solution: Checkpoint Manager

Layer 1 introduces a **Checkpoint Manager** that handles all checkpoint operations automatically based on the drive's `CommitStrategy`.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Layer 1 Architecture with Checkpoint Manager                            │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                     FileSystemClient                              │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │  │
│  │  │   Drive     │  │    File     │  │    Checkpoint           │  │  │
│  │  │  Manager    │  │   Manager   │  │    Manager              │  │  │
│  │  └─────────────┘  └─────────────┘  └─────────────────────────┘  │  │
│  │                                              │                    │  │
│  │                                              ▼                    │  │
│  │                                    ┌─────────────────────┐       │  │
│  │                                    │ Provider Registry   │       │  │
│  │                                    │ (endpoints cache)   │       │  │
│  │                                    └─────────────────────┘       │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                              │                                          │
│                              ▼                                          │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                      Layer 0 (Storage)                            │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │  │
│  │  │  Provider   │  │  Provider   │  │      Blockchain         │  │  │
│  │  │   Node A    │  │   Node B    │  │       (Pallet)          │  │  │
│  │  └─────────────┘  └─────────────┘  └─────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Checkpoint Manager Design

### Responsibilities

1. **Provider Discovery**: Automatically discover provider endpoints for a bucket
2. **Commitment Collection**: Query all providers for their current commitment
3. **Consensus Verification**: Ensure providers agree on the same state
4. **Signature Aggregation**: Collect signatures from agreeing providers
5. **On-Chain Submission**: Submit checkpoint transaction
6. **Retry & Recovery**: Handle transient failures gracefully
7. **Conflict Resolution**: Handle provider disagreements

### Data Structures

```rust
/// Checkpoint Manager configuration
pub struct CheckpointConfig {
    /// Maximum time to wait for provider responses
    pub provider_timeout: Duration,
    /// Number of retries for failed provider queries
    pub max_retries: u32,
    /// Delay between retries (exponential backoff base)
    pub retry_delay: Duration,
    /// Minimum percentage of providers that must agree (default: 51%)
    pub consensus_threshold: Percent,
    /// Whether to auto-submit checkpoints based on CommitStrategy
    pub auto_submit: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            provider_timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay: Duration::from_secs(2),
            consensus_threshold: Percent::from_percent(51),
            auto_submit: true,
        }
    }
}

/// Provider information cached by the Checkpoint Manager
pub struct ProviderInfo {
    pub account_id: AccountId,
    pub endpoint: String,
    pub public_key: Vec<u8>,
    pub last_seen: Instant,
    pub status: ProviderStatus,
}

pub enum ProviderStatus {
    Healthy,
    Degraded { last_error: String },
    Unreachable { since: Instant },
}

/// Result of commitment collection
pub struct CommitmentCollection {
    pub bucket_id: BucketId,
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub signatures: Vec<(AccountId, MultiSignature)>,
    pub agreeing_providers: Vec<AccountId>,
    pub disagreeing_providers: Vec<(AccountId, H256)>, // (provider, their_mmr_root)
    pub unreachable_providers: Vec<AccountId>,
}

/// Checkpoint submission result
pub enum CheckpointResult {
    /// Checkpoint submitted successfully
    Submitted {
        block_hash: H256,
        signers: Vec<AccountId>,
    },
    /// Not enough providers agreed (below threshold)
    InsufficientConsensus {
        agreeing: usize,
        required: usize,
        disagreements: Vec<(AccountId, H256)>,
    },
    /// All providers unreachable
    ProvidersUnreachable {
        providers: Vec<AccountId>,
    },
    /// Transaction failed
    TransactionFailed {
        error: String,
    },
}
```

---

## Protocol Specification

### Phase 1: Provider Discovery

When a drive is created or accessed, the Checkpoint Manager discovers providers:

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Provider Discovery Protocol                                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. Query on-chain: StorageAgreements for bucket_id                     │
│     → Get list of provider AccountIds                                    │
│                                                                          │
│  2. Query on-chain: Providers storage map                                │
│     → Get multiaddr (endpoint) for each provider                        │
│     → Get public_key for signature verification                         │
│                                                                          │
│  3. Cache provider info locally                                          │
│     → Refresh periodically (e.g., every 5 minutes)                      │
│     → Refresh on checkpoint failure                                      │
│                                                                          │
│  4. Health check providers                                               │
│     → GET /health to verify reachability                                │
│     → Track status (Healthy/Degraded/Unreachable)                       │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

```rust
impl CheckpointManager {
    /// Discover providers for a bucket from on-chain state
    pub async fn discover_providers(&mut self, bucket_id: BucketId) -> Result<Vec<ProviderInfo>> {
        // 1. Get bucket info
        let bucket = self.chain_client
            .query_bucket(bucket_id)
            .await?;

        // 2. Get agreements to find providers
        let agreements = self.chain_client
            .query_bucket_agreements(bucket_id)
            .await?;

        let mut providers = Vec::new();

        for agreement in agreements {
            if matches!(agreement.role, ProviderRole::Primary) {
                // 3. Get provider details
                let provider_info = self.chain_client
                    .query_provider(&agreement.provider)
                    .await?;

                // 4. Parse multiaddr to HTTP endpoint
                let endpoint = parse_multiaddr_to_http(&provider_info.multiaddr)?;

                providers.push(ProviderInfo {
                    account_id: agreement.provider,
                    endpoint,
                    public_key: provider_info.public_key,
                    last_seen: Instant::now(),
                    status: ProviderStatus::Healthy,
                });
            }
        }

        // 5. Cache for future use
        self.provider_cache.insert(bucket_id, providers.clone());

        Ok(providers)
    }
}
```

### Phase 2: Commitment Collection

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Commitment Collection Protocol                                          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  For each provider (in parallel):                                        │
│                                                                          │
│  1. Send: GET /commitment?bucket_id=X                                   │
│     With timeout: 30 seconds (configurable)                             │
│                                                                          │
│  2. Receive: {                                                          │
│       mmr_root: H256,                                                   │
│       start_seq: u64,                                                   │
│       leaf_count: u64,                                                  │
│       provider_signature: MultiSignature                                │
│     }                                                                   │
│                                                                          │
│  3. Verify signature locally:                                           │
│     payload = CommitmentPayload::new(bucket_id, mmr_root, start_seq, 0) │
│     verify(signature, payload.encode(), provider.public_key)            │
│                                                                          │
│  4. On timeout/error: Retry up to max_retries with exponential backoff  │
│                                                                          │
│  5. Categorize results:                                                 │
│     - Success: Add to collection                                        │
│     - Timeout: Mark provider as degraded, retry                         │
│     - Error: Log, mark provider status                                  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

```rust
impl CheckpointManager {
    /// Collect commitments from all providers for a bucket
    pub async fn collect_commitments(&self, bucket_id: BucketId) -> Result<CommitmentCollection> {
        let providers = self.get_cached_providers(bucket_id)
            .or_else(|| self.discover_providers(bucket_id).await)?;

        // Query all providers in parallel
        let futures: Vec<_> = providers.iter()
            .map(|p| self.query_provider_commitment(p, bucket_id))
            .collect();

        let results = futures::future::join_all(futures).await;

        // Categorize results
        let mut commitments: HashMap<H256, Vec<(AccountId, CommitmentResponse)>> = HashMap::new();
        let mut unreachable = Vec::new();

        for (provider, result) in providers.iter().zip(results) {
            match result {
                Ok(commitment) => {
                    // Verify signature before accepting
                    if self.verify_commitment_signature(&commitment, provider)? {
                        commitments
                            .entry(commitment.mmr_root)
                            .or_default()
                            .push((provider.account_id.clone(), commitment));
                    }
                }
                Err(_) => {
                    unreachable.push(provider.account_id.clone());
                }
            }
        }

        // Find majority consensus
        let (majority_root, agreeing) = commitments
            .iter()
            .max_by_key(|(_, v)| v.len())
            .map(|(root, v)| (*root, v.clone()))
            .unwrap_or_default();

        // Build result
        let disagreeing: Vec<_> = commitments
            .iter()
            .filter(|(root, _)| **root != majority_root)
            .flat_map(|(root, providers)| {
                providers.iter().map(|(id, _)| (id.clone(), *root))
            })
            .collect();

        Ok(CommitmentCollection {
            bucket_id,
            mmr_root: majority_root,
            start_seq: agreeing.first().map(|(_, c)| c.start_seq).unwrap_or(0),
            leaf_count: agreeing.first().map(|(_, c)| c.leaf_count).unwrap_or(0),
            signatures: agreeing.iter()
                .map(|(id, c)| (id.clone(), c.provider_signature.clone()))
                .collect(),
            agreeing_providers: agreeing.iter().map(|(id, _)| id.clone()).collect(),
            disagreeing_providers: disagreeing,
            unreachable_providers: unreachable,
        })
    }

    async fn query_provider_commitment(
        &self,
        provider: &ProviderInfo,
        bucket_id: BucketId,
    ) -> Result<CommitmentResponse> {
        let mut retries = 0;
        let mut delay = self.config.retry_delay;

        loop {
            let result = tokio::time::timeout(
                self.config.provider_timeout,
                self.http_client
                    .get(format!("{}/commitment?bucket_id={}", provider.endpoint, bucket_id))
                    .send()
            ).await;

            match result {
                Ok(Ok(response)) => {
                    return response.json::<CommitmentResponse>().await
                        .map_err(|e| Error::ProviderResponse(e.to_string()));
                }
                _ if retries < self.config.max_retries => {
                    retries += 1;
                    tokio::time::sleep(delay).await;
                    delay *= 2; // Exponential backoff
                }
                Ok(Err(e)) => return Err(Error::ProviderUnreachable(e.to_string())),
                Err(_) => return Err(Error::ProviderTimeout),
            }
        }
    }
}
```

### Phase 3: Consensus Verification & Submission

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Consensus & Submission Protocol                                         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. Check consensus threshold:                                          │
│     agreeing_count >= total_providers × consensus_threshold             │
│                                                                          │
│  2. If below threshold:                                                 │
│     a) If disagreeing providers exist → Log warning, return conflict    │
│     b) If only unreachable → Retry or wait                              │
│                                                                          │
│  3. If above threshold:                                                 │
│     a) Build extrinsic: submit_commitment(...)                          │
│     b) Sign with user's keypair                                         │
│     c) Submit to chain                                                  │
│     d) Wait for finalization                                            │
│     e) Verify event emitted                                             │
│                                                                          │
│  4. On success:                                                         │
│     - Update local cache with new snapshot                              │
│     - Clear pending changes queue                                       │
│     - Emit success event/callback                                       │
│                                                                          │
│  5. On failure:                                                         │
│     - Log error with details                                            │
│     - Schedule retry based on CommitStrategy                            │
│     - Emit failure event/callback                                       │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

```rust
impl CheckpointManager {
    /// Submit a checkpoint for a bucket
    pub async fn submit_checkpoint(&self, bucket_id: BucketId) -> CheckpointResult {
        // 1. Collect commitments
        let collection = match self.collect_commitments(bucket_id).await {
            Ok(c) => c,
            Err(e) => return CheckpointResult::TransactionFailed {
                error: e.to_string()
            },
        };

        // 2. Check if all providers unreachable
        if collection.agreeing_providers.is_empty() {
            return CheckpointResult::ProvidersUnreachable {
                providers: collection.unreachable_providers,
            };
        }

        // 3. Check consensus threshold
        let total_providers = collection.agreeing_providers.len()
            + collection.disagreeing_providers.len()
            + collection.unreachable_providers.len();

        let required = (total_providers as f64 * self.config.consensus_threshold.deconstruct() as f64 / 100.0).ceil() as usize;

        if collection.agreeing_providers.len() < required {
            return CheckpointResult::InsufficientConsensus {
                agreeing: collection.agreeing_providers.len(),
                required,
                disagreements: collection.disagreeing_providers,
            };
        }

        // 4. Submit on-chain
        match self.chain_client.submit_commitment(
            collection.bucket_id,
            collection.mmr_root,
            collection.start_seq,
            collection.leaf_count,
            collection.signatures,
        ).await {
            Ok(block_hash) => CheckpointResult::Submitted {
                block_hash,
                signers: collection.agreeing_providers,
            },
            Err(e) => CheckpointResult::TransactionFailed {
                error: e.to_string(),
            },
        }
    }
}
```

---

## Integration with CommitStrategy

The Checkpoint Manager respects the drive's `CommitStrategy`:

### Immediate Strategy

```rust
impl FileSystemClient {
    pub async fn upload_file(&mut self, drive_id: DriveId, path: &str, data: &[u8]) -> Result<()> {
        // ... upload logic ...

        // Check commit strategy
        let drive = self.get_drive_info(drive_id).await?;

        if matches!(drive.commit_strategy, CommitStrategy::Immediate) {
            // Submit checkpoint immediately
            let result = self.checkpoint_manager
                .submit_checkpoint(drive.bucket_id)
                .await;

            match result {
                CheckpointResult::Submitted { .. } => {
                    // Update on-chain root CID
                    self.update_root_cid(drive_id, new_root_cid).await?;
                }
                CheckpointResult::InsufficientConsensus { .. } => {
                    log::warn!("Checkpoint delayed: insufficient consensus");
                    // Queue for retry
                }
                _ => { /* Handle other cases */ }
            }
        }

        Ok(())
    }
}
```

### Batched Strategy

```rust
impl CheckpointManager {
    /// Background task for batched checkpoints
    pub async fn run_batched_checkpoint_loop(&self) {
        loop {
            // Wait for next interval
            tokio::time::sleep(Duration::from_secs(6)).await; // ~1 block

            let current_block = self.chain_client.current_block().await;

            // Check all drives with batched strategy
            for (drive_id, drive_info) in self.drives_cache.iter() {
                if let CommitStrategy::Batched { interval } = drive_info.commit_strategy {
                    let blocks_since_last = current_block - drive_info.last_committed_at;

                    if blocks_since_last >= interval as u64 {
                        // Check if there are pending changes
                        if self.has_pending_changes(drive_id) {
                            log::info!("Submitting batched checkpoint for drive {}", drive_id);
                            let _ = self.submit_checkpoint(drive_info.bucket_id).await;
                        }
                    }
                }
            }
        }
    }
}
```

### Manual Strategy

```rust
impl FileSystemClient {
    /// Manually trigger checkpoint (for Manual strategy)
    pub async fn commit_changes(&mut self, drive_id: DriveId) -> Result<CheckpointResult> {
        let drive = self.get_drive_info(drive_id).await?;

        // Submit checkpoint
        let result = self.checkpoint_manager
            .submit_checkpoint(drive.bucket_id)
            .await;

        if let CheckpointResult::Submitted { .. } = &result {
            // Get new root CID from pending changes
            let new_root_cid = self.pending_root_cid(drive_id)?;

            // Update on-chain
            self.update_root_cid(drive_id, new_root_cid).await?;
        }

        Ok(result)
    }
}
```

---

## Conflict Resolution Protocol

When providers disagree, the system follows a resolution protocol:

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Conflict Resolution Protocol                                            │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Scenario: Provider A has mmr_root=0xABC, Provider B has mmr_root=0xDEF │
│                                                                          │
│  Step 1: Determine which is "ahead"                                     │
│  ─────────────────────────────────────                                   │
│  Compare leaf_count:                                                    │
│  - If A.leaf_count > B.leaf_count → A is ahead, B needs to sync        │
│  - If B.leaf_count > A.leaf_count → B is ahead, A needs to sync        │
│  - If equal → Different data (potential corruption)                    │
│                                                                          │
│  Step 2: Wait for sync (if one is behind)                               │
│  ────────────────────────────────────────                                │
│  - Wait sync_interval blocks                                            │
│  - Re-query commitments                                                 │
│  - If now agree → Proceed                                               │
│  - If still disagree → Escalate                                         │
│                                                                          │
│  Step 3: Escalate (if true conflict)                                    │
│  ───────────────────────────────────                                     │
│  - Log detailed warning                                                 │
│  - Notify user/admin                                                    │
│  - Options:                                                             │
│    a) Submit with majority (if above threshold)                         │
│    b) Wait for manual intervention                                      │
│    c) Challenge the disagreeing provider                                │
│                                                                          │
│  Step 4: Challenge (optional)                                           │
│  ────────────────────────────────                                        │
│  If provider claims to have data they shouldn't:                        │
│  - Use challenge_offchain with majority's signature                     │
│  - Force provider to prove or be slashed                                │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

```rust
impl CheckpointManager {
    async fn handle_conflict(
        &self,
        bucket_id: BucketId,
        collection: &CommitmentCollection,
    ) -> ConflictResolution {
        // Analyze the conflict
        let majority_leaf_count = collection.leaf_count;

        for (provider_id, their_root) in &collection.disagreeing_providers {
            // Query their full commitment to get leaf_count
            let their_commitment = self.query_provider_commitment_by_id(provider_id, bucket_id).await;

            if let Ok(commitment) = their_commitment {
                if commitment.leaf_count < majority_leaf_count {
                    // They're behind - likely sync delay
                    log::info!(
                        "Provider {} is {} leaves behind, likely sync delay",
                        provider_id,
                        majority_leaf_count - commitment.leaf_count
                    );
                    return ConflictResolution::WaitForSync {
                        provider: provider_id.clone(),
                        behind_by: majority_leaf_count - commitment.leaf_count,
                    };
                } else if commitment.leaf_count == majority_leaf_count {
                    // Same leaf count but different root - data divergence!
                    log::warn!(
                        "Provider {} has different data at same leaf count! Potential corruption.",
                        provider_id
                    );
                    return ConflictResolution::DataDivergence {
                        provider: provider_id.clone(),
                        majority_root: collection.mmr_root,
                        their_root: *their_root,
                    };
                }
            }
        }

        ConflictResolution::ProceedWithMajority
    }
}

enum ConflictResolution {
    ProceedWithMajority,
    WaitForSync { provider: AccountId, behind_by: u64 },
    DataDivergence { provider: AccountId, majority_root: H256, their_root: H256 },
}
```

---

## User-Facing API

With this protocol, the user API becomes simple:

```rust
// User doesn't need to know about checkpoints!
let mut fs_client = FileSystemClient::new(
    "ws://localhost:2222",
    "http://localhost:3333",
).await?
    .with_dev_signer("alice").await?;

// Create drive - checkpoints handled automatically based on strategy
let drive_id = fs_client.create_drive(
    Some("My Files"),
    10_000_000_000,    // 10 GB
    500,               // 500 blocks
    1_000_000_000_000, // 1 token
    None,              // Auto providers
    Some(CommitStrategy::Batched { interval: 100 }), // Checkpoint every 100 blocks
).await?;

// Upload file - checkpoint submitted automatically (if Immediate)
// or queued for batch submission
fs_client.upload_file(drive_id, "/document.pdf", &data).await?;

// For Manual strategy only:
if let CommitStrategy::Manual = drive_info.commit_strategy {
    // User explicitly triggers checkpoint
    let result = fs_client.commit_changes(drive_id).await?;
    match result {
        CheckpointResult::Submitted { signers, .. } => {
            println!("Checkpoint submitted with {} signers", signers.len());
        }
        CheckpointResult::InsufficientConsensus { agreeing, required, .. } => {
            println!("Only {}/{} providers agreed, waiting...", agreeing, required);
        }
        _ => { /* handle other cases */ }
    }
}

// Optional: Check checkpoint status
let status = fs_client.checkpoint_status(drive_id).await?;
println!("Last checkpoint: block {}", status.last_checkpoint_block);
println!("Pending changes: {}", status.has_pending_changes);
println!("Provider health: {:?}", status.provider_health);
```

---

## Events & Callbacks

For applications that need visibility:

```rust
// Subscribe to checkpoint events
fs_client.on_checkpoint_submitted(|event| {
    println!("Checkpoint submitted for drive {}", event.drive_id);
    println!("  MMR Root: {:?}", event.mmr_root);
    println!("  Signers: {:?}", event.signers);
});

fs_client.on_checkpoint_failed(|event| {
    println!("Checkpoint failed for drive {}", event.drive_id);
    println!("  Reason: {:?}", event.reason);
    println!("  Will retry: {}", event.will_retry);
});

fs_client.on_provider_conflict(|event| {
    println!("Provider conflict detected!");
    println!("  Majority: {:?}", event.majority_root);
    println!("  Disagreeing: {:?}", event.disagreeing_providers);
});
```

---

## Implementation Phases

### Phase 1: Core Protocol (MVP) ✅
- [x] Provider discovery from on-chain state
- [x] Parallel commitment collection
- [x] Basic consensus verification (majority)
- [x] Checkpoint submission
- [x] Integration with CommitStrategy

### Phase 2: Reliability ✅
- [x] Retry with exponential backoff
- [x] Provider health tracking (`ProviderHealthHistory`)
- [x] Conflict detection and logging (`ProviderConflict`, `ConflictType`)
- [x] Background batched checkpoint loop (`CheckpointLoopHandle`)

### Phase 3: Advanced
- [x] Conflict resolution protocol (`ConflictResolution`)
- [ ] Automatic challenge for divergent providers
- [x] Event/callback system (`CheckpointCallback`)
- [ ] Metrics and monitoring

---

## Implemented API Reference

The following types and functions are available in the `storage_client` crate:

### Core Types

```rust
use storage_client::{
    // Configuration
    CheckpointConfig,
    BatchedCheckpointConfig,
    BatchedInterval,

    // Manager
    CheckpointManager,
    CheckpointLoopHandle,

    // Results
    CheckpointResult,
    CommitmentCollection,
    BucketCheckpointStatus,

    // Provider Info
    ProviderInfo,
    ProviderStatus,
    ProviderHealthHistory,

    // Conflict Detection
    ProviderConflict,
    ConflictType,
    ConflictResolution,
    ConflictingProvider,

    // Callbacks
    CheckpointCallback,
    CheckpointLoopCommand,
};
```

### CheckpointManager Usage

```rust
use storage_client::{CheckpointManager, CheckpointConfig, CheckpointResult};

// Create manager
let manager = CheckpointManager::new("ws://localhost:2222", CheckpointConfig::default())
    .await?
    .with_providers(vec!["http://localhost:3333".to_string()])
    .with_dev_signer("alice")?;

// Submit checkpoint
let result = manager.submit_checkpoint(bucket_id).await;
match result {
    CheckpointResult::Submitted { block_hash, signers } => {
        println!("Checkpoint submitted at {:?} with {} signers", block_hash, signers.len());
    }
    CheckpointResult::InsufficientConsensus { agreeing, required, .. } => {
        println!("Only {}/{} providers agreed", agreeing, required);
    }
    CheckpointResult::ProvidersUnreachable { providers } => {
        println!("{} providers unreachable", providers.len());
    }
    CheckpointResult::NoProviders => {
        println!("No providers configured");
    }
    CheckpointResult::TransactionFailed { error } => {
        println!("Transaction failed: {}", error);
    }
}
```

### Background Checkpoint Loop

```rust
use storage_client::{
    CheckpointManager, BatchedCheckpointConfig, BatchedInterval, CheckpointCallback,
};
use std::sync::Arc;

// Create manager wrapped in Arc for background loop
let manager = Arc::new(CheckpointManager::new("ws://localhost:2222", CheckpointConfig::default())
    .await?
    .with_providers(vec!["http://localhost:3333".to_string()])
    .with_dev_signer("alice")?);

// Configure batched checkpoints
let config = BatchedCheckpointConfig {
    interval: BatchedInterval::Blocks(100), // Every 100 blocks
    submit_on_empty: false,                 // Only checkpoint if changes exist
    max_consecutive_failures: 5,            // Pause after 5 failures
    failure_retry_delay: Duration::from_secs(30),
};

// Optional callback for checkpoint events
let callback: Option<CheckpointCallback> = Some(Arc::new(|bucket_id, result| {
    match result {
        CheckpointResult::Submitted { .. } => {
            println!("Checkpoint submitted for bucket {}", bucket_id);
        }
        _ => {
            println!("Checkpoint failed for bucket {}: {:?}", bucket_id, result);
        }
    }
}));

// Start background loop
let mut handle = manager.start_checkpoint_loop(bucket_id, config, callback).await?;

// Control the loop
handle.mark_dirty(bucket_id).await?;  // Mark bucket as having changes
handle.submit_now().await?;           // Force immediate checkpoint
handle.pause().await?;                // Pause the loop
handle.resume().await?;               // Resume the loop

// Check if running
if handle.is_running() {
    println!("Checkpoint loop is active");
}

// Stop when done
handle.stop().await?;
```

### Provider Health Tracking

```rust
use storage_client::ProviderHealthHistory;

// Health history is automatically tracked by CheckpointManager
let history = manager.get_health_history(&provider_account_id).await;

if let Some(h) = history {
    println!("Provider health:");
    println!("  Total requests: {}", h.total_requests);
    println!("  Success rate: {:.1}%", h.success_rate() * 100.0);
    println!("  Consecutive failures: {}", h.consecutive_failures);
    println!("  Status: {:?}", h.current_status());
    println!("  Is healthy: {}", h.is_healthy());
}

// Get providers sorted by health
let healthy_providers = manager.get_providers_by_health(bucket_id).await?;

// Check if enough healthy providers for consensus
let can_checkpoint = manager.has_enough_healthy_providers(bucket_id).await?;
```

### Conflict Detection

```rust
use storage_client::{ConflictType, ConflictResolution};

// Collect commitments with conflict analysis
let (collection, conflict) = manager.collect_commitments_with_conflicts(bucket_id).await?;

if let Some(conflict) = conflict {
    println!("Conflict detected!");
    println!("  Majority root: {:?}", conflict.majority_root);
    println!("  Majority count: {}", conflict.majority_count);

    for c in &conflict.conflicts {
        match &c.conflict_type {
            ConflictType::SyncDelay { behind_by } => {
                println!("  {} is {} leaves behind (sync delay)", c.account_id, behind_by);
            }
            ConflictType::DataDivergence => {
                println!("  {} has different data (potential corruption)", c.account_id);
            }
            ConflictType::Ahead { ahead_by } => {
                println!("  {} is {} leaves ahead", c.account_id, ahead_by);
            }
        }
    }

    match &conflict.resolution {
        ConflictResolution::WaitForSync { estimated_blocks } => {
            println!("  Resolution: Wait ~{} blocks for sync", estimated_blocks);
        }
        ConflictResolution::ProceedWithMajority => {
            println!("  Resolution: Proceed with majority");
        }
        ConflictResolution::ConsiderChallenge { provider } => {
            println!("  Resolution: Consider challenging {}", provider);
        }
        ConflictResolution::ManualIntervention { reason } => {
            println!("  Resolution: Manual intervention needed - {}", reason);
        }
    }
}
```

### FileSystemClient Integration

```rust
use file_system_client::FileSystemClient;

let mut fs_client = FileSystemClient::new("ws://localhost:2222", "http://localhost:3333")
    .await?
    .with_dev_signer("alice").await?;

// Enable automatic checkpoints for a drive
fs_client.enable_auto_checkpoints(
    drive_id,
    vec!["http://localhost:3333".to_string()],
    Some(100),  // Checkpoint every 100 blocks
    Some(Arc::new(|bucket_id, result| {
        println!("Auto-checkpoint for bucket {}: {:?}", bucket_id, result);
    })),
).await?;

// File operations automatically mark drive as dirty
fs_client.upload_file(drive_id, "/document.txt", data.as_bytes(), bucket_id).await?;
fs_client.create_directory(drive_id, "/folder", bucket_id).await?;

// Force immediate checkpoint
fs_client.request_immediate_checkpoint().await?;

// Check status
if fs_client.is_auto_checkpoints_enabled() {
    println!("Auto-checkpoints are active");
}

// Manual checkpoint (for drives with Manual commit strategy)
let result = fs_client.submit_checkpoint(drive_id, vec!["http://localhost:3333".to_string()]).await?;

// Disable when done
fs_client.disable_auto_checkpoints().await?;
```

---

## Summary

| Before (Manual) | After (Automated) |
|-----------------|-------------------|
| User queries each provider | Client discovers providers automatically |
| User verifies mmr_root match | Client handles consensus verification |
| User collects all signatures | Client aggregates signatures |
| User handles disagreements | Client resolves conflicts |
| User submits transaction | Client submits based on CommitStrategy |
| User retries on failure | Client retries with backoff |

**User experience**: Upload files → System handles checkpoints → Data is secure

---

## Related Documents

- [Execution Flows](./EXECUTION_FLOWS.md) - Detailed sequence diagrams
- [Architecture](../filesystems/ARCHITECTURE.md) - System architecture
- [API Reference](../filesystems/API_REFERENCE.md) - Complete API docs
