# Layer 1 File System Architecture

## Overview

Layer 1 acts as the **orchestration layer** between users and Layer 0 storage, managing:
- **Agreement tracking**: Links drives to storage agreements
- **Commit strategies**: Controls when changes go on-chain
- **Dispute coordination**: Tracks failed challenges
- **Provider management**: Handles provider replacements

## Design Principles

### 1. User Control
Users decide:
- **When to commit**: Immediate, batched, or manual
- **Storage preferences**: Budget, providers, redundancy level
- **Dispute handling**: When to raise disputes

### 2. Separation of Concerns
- **Layer 0**: Raw storage (buckets, agreements, challenges)
- **Layer 1**: Orchestration (drives, batching, monitoring)
- **Layer 2**: User interfaces (FUSE, web UI, CLI)

### 3. Cost Optimization
- Batch commits reduce transaction costs
- Pending changes tracked off-chain
- Only final root CID goes on-chain

## Key Components

### Enhanced DriveInfo

```rust
pub struct DriveInfo {
    owner: AccountId,
    bucket_id: u64,
    agreement_ids: BoundedVec<AgreementId, MaxAgreements>,  // NEW
    root_cid: Cid,                    // Committed root
    pending_root_cid: Option<Cid>,    // NEW: Uncommitted changes
    commit_strategy: CommitStrategy,   // NEW
    created_at: BlockNumber,
    last_committed_at: BlockNumber,    // NEW
    name: Option<BoundedVec<u8, MaxNameLength>>,
}
```

### Commit Strategies

```rust
pub enum CommitStrategy {
    Immediate,                    // Every change → on-chain (expensive)
    Batched { interval: u32 },    // Commit every N blocks
    Manual,                       // User explicitly commits
}
```

**Cost Comparison:**
- **Immediate**: 1 tx per operation = High cost, real-time updates
- **Batched (100 blocks)**: 1 tx per ~10 min = Medium cost, near-real-time
- **Manual**: User controlled = Low cost, controlled timing

### New Extrinsics

#### 1. create_drive_with_storage

Creates a drive linked to existing Layer 0 agreements.

```rust
pub fn create_drive_with_storage(
    bucket_id: u64,
    agreement_ids: Vec<AgreementId>,
    batched_commits: bool,
    batch_interval: u32,
    root_cid: Cid,
    name: Option<Vec<u8>>,
)
```

**Workflow:**
1. User creates bucket in Layer 0: `storage_provider.create_bucket()`
2. User requests agreements: `storage_provider.request_agreement()` × N
3. User creates drive: `drive_registry.create_drive_with_storage()`

**Why separate?**
- Modular: Each layer handles its concerns
- Flexible: Users can manage agreements independently
- No inter-pallet calls: Avoids complexity

#### 2. commit_changes

Commits pending root CID to on-chain state.

```rust
pub fn commit_changes(drive_id: DriveId)
```

**Used with:**
- Manual strategy: User decides when
- Batched strategy: Called automatically by off-chain worker

#### 3. raise_drive_dispute

Tracks disputes at the drive level.

```rust
pub fn raise_drive_dispute(
    drive_id: DriveId,
    agreement_id: AgreementId,
    challenge_id: u64,
)
```

**Workflow:**
1. Challenge issued in Layer 0
2. Monitor detects failure
3. Calls this to track at drive level
4. Emits `DisputeRaised` event
5. User/monitor can `replace_provider`

#### 4. replace_provider

Swaps failed provider with new one.

```rust
pub fn replace_provider(
    drive_id: DriveId,
    failed_agreement_id: AgreementId,
    new_agreement_id: AgreementId,
)
```

**Workflow:**
1. Dispute resolved (provider slashed)
2. User creates new agreement in Layer 0
3. Calls this to update drive's agreement list

## Data Flow

### File Upload with Batching

```
┌─────────────────────────────────────────────────────────┐
│ Client (Off-chain)                                      │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  1. upload_file("/docs/file1.txt")                     │
│     ├─> Split into chunks                               │
│     ├─> Upload chunks to Layer 0                        │
│     ├─> Create FileManifest                             │
│     ├─> Update parent DirectoryNode                     │
│     ├─> Calculate new root CID                          │
│     └─> Store pending_root_cid locally                  │
│                                                          │
│  2. upload_file("/docs/file2.txt")                     │
│     └─> Same process, updates pending_root_cid         │
│                                                          │
│  3. create_directory("/images")                        │
│     └─> Updates pending_root_cid again                 │
│                                                          │
└─────────────────────────────────────────────────────────┘
                          │
                          │ (Batched: After 100 blocks)
                          │ (Manual: User calls commit())
                          ▼
┌─────────────────────────────────────────────────────────┐
│ On-Chain (Layer 1)                                      │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  commit_changes(drive_id)                              │
│  ├─> root_cid = pending_root_cid                       │
│  ├─> pending_root_cid = None                           │
│  ├─> last_committed_at = current_block                 │
│  └─> Emit ChangesCommitted event                       │
│                                                          │
│  Single transaction for all 3 operations!              │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### Challenge Monitoring & Disputes

```
┌──────────────────────────────────────────────────────┐
│ Layer 0: Storage Provider Pallet                    │
├──────────────────────────────────────────────────────┤
│  Challenge issued                                    │
│  ├─> Provider has 48 hours to respond                │
│  └─> Emit ChallengeIssued event                      │
└──────────────────────────────────────────────────────┘
                    │
                    │ (Monitored by)
                    ▼
┌──────────────────────────────────────────────────────┐
│ Storage Monitor Service (Off-chain Worker)           │
├──────────────────────────────────────────────────────┤
│  Watches Layer 0 events:                            │
│  ├─> ChallengeIssued                                 │
│  ├─> ChallengeResponded                              │
│  └─> ChallengeTimeout                                │
│                                                       │
│  If provider fails:                                  │
│  1. Verify proof is invalid / timeout occurred       │
│  2. Call drive_registry.raise_drive_dispute()        │
│  3. Notify user                                      │
│  4. Optionally: Auto-replace provider                │
└──────────────────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────────────────┐
│ Layer 1: Drive Registry                              │
├──────────────────────────────────────────────────────┤
│  raise_drive_dispute(drive_id, agreement_id)        │
│  └─> Emit DisputeRaised event                        │
│                                                       │
│  User/Monitor calls:                                 │
│  replace_provider(drive_id, old_id, new_id)         │
│  └─> Updates agreement_ids in DriveInfo              │
└──────────────────────────────────────────────────────┘
```

## Decision Points

### When to Create Drive?

**Option 1: create_drive (Simple)**
```rust
// User manages everything manually
storage_provider.create_bucket(min_providers=3);
storage_provider.request_agreement(...); // × 3
drive_registry.create_drive(bucket_id, root_cid, name);
```

**Option 2: create_drive_with_storage (Orchestrated)**
```rust
// User provides preferences, Layer 1 tracks agreements
storage_provider.create_bucket(min_providers=3);
storage_provider.request_agreement(...); // × 3
drive_registry.create_drive_with_storage(
    bucket_id,
    [agreement_1, agreement_2, agreement_3],
    batched_commits=true,
    batch_interval=100,
    root_cid,
    name,
);
```

**Recommendation**: Use Option 2 for better tracking.

### When to Commit?

| Strategy | Use Case | Cost | Latency |
|----------|----------|------|---------|
| **Immediate** | Critical data, audit trail | High | None |
| **Batched** | Normal files, collaborative editing | Medium | ~10 min |
| **Manual** | Bulk uploads, git-like workflow | Low | User-controlled |

**Example scenarios:**

1. **Medical Records** → Immediate
   - Every change must be timestamped on-chain
   - Regulatory requirements

2. **Collaborative Docs** → Batched (100 blocks)
   - Balance freshness and cost
   - Acceptable ~10 min delay

3. **Data Science** → Manual
   - Upload 1000 files
   - Commit once at end

### When to Raise Disputes?

**Automated (Recommended):**
```rust
// Monitor service watches challenges
if challenge.timeout_reached() || !challenge.proof_valid() {
    drive_registry.raise_drive_dispute(drive_id, agreement_id, challenge_id);
}
```

**Manual:**
```rust
// User manually raises after reviewing
drive_registry.raise_drive_dispute(drive_id, agreement_id, challenge_id);
```

**Recommendation**: Use automated monitoring for reliability.

## Implementation Status

### ✅ Completed
- Enhanced primitives with `CommitStrategy` and `DriveConfig`
- Updated `DriveInfo` with agreement tracking
- New extrinsics: `create_drive_with_storage`, `commit_changes`, `raise_drive_dispute`, `replace_provider`
- Runtime integration with `MaxAgreements` constant
- Pallet compiles successfully

### 🚧 In Progress
- Storage monitor service (off-chain worker)
- FileSystemClient batching support
- Comprehensive tests
- Updated examples

### 📋 Planned
- Auto-commit based on strategy (off-chain worker)
- Provider reputation tracking
- Automatic provider replacement
- Cost estimation API

## Security Considerations

### 1. Agreement Verification
- Users must verify agreements exist before creating drive
- No automatic validation (by design - flexibility)
- Consider adding optional verification extrinsic

### 2. Dispute Handling
- Disputes tracked but not automatically processed
- Users responsible for monitoring (or use monitor service)
- Consider slashing penalties for false disputes

### 3. Access Control
- Only drive owner can commit changes
- Only drive owner can raise disputes
- No delegation mechanism (future enhancement)

## Performance Considerations

### Storage Costs
- Each drive: ~200 bytes on-chain
- Agreement list: 8 bytes × N providers
- Pending CID: 32 bytes (optional)
- Total: ~250 bytes per drive

### Transaction Costs
- `create_drive`: ~10,000 weight
- `commit_changes`: ~10,000 weight
- Batching 100 operations: 100× savings

### Scalability
- Max drives per user: 100 (configurable)
- Max agreements per drive: 10 (configurable)
- No limit on total drives system-wide

## Future Enhancements

### 1. Automatic Committing
Off-chain worker that:
- Watches drives with `Batched` strategy
- Commits when interval reached
- Handles failures gracefully

### 2. Smart Provider Selection
```rust
fn select_providers(
    budget: Balance,
    storage_size: u64,
    preferences: ProviderPreferences,
) -> Vec<(AccountId, Balance)> {
    // Consider:
    // - Reputation score
    // - Geographic distribution
    // - Pricing
    // - Capacity
    // - Uptime history
}
```

### 3. Multi-User Drives
```rust
pub struct DriveAccess {
    account: AccountId,
    role: DriveRole,  // Owner, Editor, Viewer
}

pub struct DriveInfo {
    // ... existing fields
    access_list: BoundedVec<DriveAccess, MaxUsers>,
}
```

### 4. Snapshots & Time Travel
```rust
pub struct DriveSnapshot {
    drive_id: DriveId,
    root_cid: Cid,
    timestamp: BlockNumber,
}

// Query drive state at any historical block
fn get_drive_at_block(drive_id: DriveId, block: BlockNumber) -> Option<Cid>
```

## References

- [Layer 0 Implementation](../../docs/design/scalable-web3-storage-implementation.md)
- [Three-Layered Architecture](../../docs/design/scalable-web3-storage.md)
- [File System Primitives](./primitives/src/lib.rs)
- [Drive Registry Pallet](./pallet-registry/src/lib.rs)
