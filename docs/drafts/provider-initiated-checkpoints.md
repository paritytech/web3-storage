# Provider-Initiated Checkpoints Design

> ⚠️ **Removed from the codebase — [#306](https://github.com/paritytech/web3-storage/issues/306).**
> This design was added and implemented without review (#7, #92) and did not
> hold up under scrutiny (see the issue: the client can checkpoint in-session,
> client-held signed commitments already provide protection, and the
> coordination overlaps replica nodes). The design was pulled from the
> review-gated `docs/design/` in #305, and the entire implementation (pallet
> extrinsics/storage/events, provider-node coordinator + HTTP endpoints, SDK
> wrappers, UI panels) was removed for #306. This document is the archive:
> the original (unvalidated) design below, an
> [as-shipped specification + re-implementation guide](#re-implementation-guide-as-shipped-specification),
> and an [Implementation Archive](#implementation-archive-code-removed-in-306)
> of the removed code, so the feature can be re-evaluated and re-implemented
> later if a validated rationale emerges. The "Problem Statement" / "Why" reasoning
> below has **not** been validated — treat with skepticism.

## Problem Statement

The current checkpoint system requires the data owner's client to:
1. Collect signatures from all providers
2. Submit the checkpoint transaction on-chain

This creates issues for:
- **Mobile users**: Apps may be closed/offline
- **Regular consumers**: No server infrastructure
- **Reliability**: Missing checkpoints means no protection

---

## Alternative Approaches Explored

Before settling on provider-initiated checkpoints, we evaluated several alternative approaches to solve the "always online" problem. This section documents each approach with its trade-offs.

### 1. Client-Initiated Checkpoints (Current/Baseline)

**How it works:** The data owner's client application coordinates checkpoint submission by collecting signatures from all providers and submitting the transaction on-chain.

```
┌──────────┐                      ┌──────────────┐
│  Client  │ ─── Get Signature ──>│  Provider A  │
│  (User)  │ ─── Get Signature ──>│  Provider B  │
│          │ ─── Submit ─────────>│  Blockchain  │
└──────────┘                      └──────────────┘
```

| Pros | Cons |
|------|------|
| Simple implementation | Client must be online |
| User has full control | Unreliable for mobile apps |
| No additional infrastructure | Missed checkpoints = no protection |
| No extra costs | Poor UX for consumers |

**Best suited for:** Server-side applications, developer tools, and scenarios where the client runs continuously.

---

### 2. Centralized Backend Server

**How it works:** Users deploy a backend server that stays online and submits checkpoints on their behalf.

```
┌────────────┐    API calls    ┌─────────────────┐
│ Mobile App │ ───────────────>│ User's Backend  │
└────────────┘                 │    Server       │
                               │  (Always On)    │
                               └────────┬────────┘
                                        │ Checkpoint
                                        ▼
                               ┌─────────────────┐
                               │   Blockchain    │
                               └─────────────────┘
```

| Pros | Cons |
|------|------|
| Simple to implement | Centralization point |
| Familiar pattern | Infrastructure cost |
| Full user control | Single point of failure |
| Can add custom logic | Defeats decentralization goal |
| | Not viable for regular consumers |
| | Server compromise = data at risk |

**Best suited for:** Enterprise users who already have infrastructure and accept centralization trade-off.

---

### 3. Substrate Offchain Workers

**How it works:** Leverage Substrate's offchain worker system to run checkpoint logic on validator nodes. Offchain workers execute code outside of block production but with access to on-chain state.

```
┌─────────────────────────────────────────────────┐
│              Validator Node                      │
│  ┌─────────────┐      ┌───────────────────────┐ │
│  │   Runtime   │      │   Offchain Worker     │ │
│  │  (On-chain) │<─────│  - Monitor buckets    │ │
│  │             │      │  - Collect signatures │ │
│  └─────────────┘      │  - Submit checkpoints │ │
│                       └───────────────────────┘ │
└─────────────────────────────────────────────────┘
```

| Pros | Cons |
|------|------|
| Uses existing infrastructure | Complex implementation |
| Decentralized execution | Validator coordination needed |
| No new network participants | Limited offchain worker capabilities |
| Integrated with chain | Can't easily contact external providers |
| | Validators may not want extra work |
| | Potential consensus issues |

**Best suited for:** Chains where validators are willing to run additional logic and provider endpoints are accessible.

---

### 4. Decentralized Keeper Network

**How it works:** A network of third-party "keepers" compete to submit checkpoints for rewards. Similar to Chainlink Keepers or Gelato Network.

```
┌────────────────┐
│   Keeper 1     │──┐
├────────────────┤  │     ┌─────────────────┐
│   Keeper 2     │──┼────>│   Blockchain    │
├────────────────┤  │     │  (First wins    │
│   Keeper 3     │──┘     │   the reward)   │
└────────────────┘        └─────────────────┘
```

| Pros | Cons |
|------|------|
| Decentralized | New network to bootstrap |
| Competition ensures reliability | Additional token economics |
| Specialization (keepers focus on this) | Keeper infrastructure costs |
| Works across many protocols | Users pay keeper fees |
| | Trust in keeper set |
| | Potential MEV/front-running |

**Best suited for:** Mature ecosystems with established keeper networks, or when building a general-purpose automation layer.

---

### 5. Challenge-Based / Lazy Checkpoints

**How it works:** Don't require regular checkpoints. Instead, anyone can challenge a provider at any time. If challenged, the provider must prove they have the data or get slashed.

```
Normal Operation (No checkpoints needed):
┌──────────┐                 ┌──────────────┐
│  Client  │ ─── Upload ────>│   Provider   │
│          │ <── Download ───│              │
└──────────┘                 └──────────────┘

Challenge (Only when suspicious):
┌──────────┐   Challenge    ┌──────────────┐
│ Challenger│ ─────────────>│  Blockchain  │
└──────────┘                └──────┬───────┘
                                   │ Respond or Slash
                                   ▼
                            ┌──────────────┐
                            │   Provider   │
                            └──────────────┘
```

| Pros | Cons |
|------|------|
| Minimal on-chain activity | No proactive verification |
| Lower costs | Relies on challengers |
| Simple protocol | Data loss discovered late |
| Provider flexibility | Challenge spam potential |
| | Complex dispute resolution |
| | Less accountability |

**Best suited for:** Low-value data, scenarios where occasional data loss is acceptable, or as a complement to other approaches.

---

### 6. Provider-Initiated Checkpoints (Recommended)

**How it works:** Providers themselves coordinate and submit checkpoints without requiring the client to be online. Detailed design follows in subsequent sections.

```
┌──────────────┐  Coordinate   ┌──────────────┐
│  Provider A  │<─────────────>│  Provider B  │
│  (Leader)    │               │              │
└──────┬───────┘               └──────────────┘
       │
       │ Submit checkpoint
       ▼
┌──────────────┐
│  Blockchain  │
└──────────────┘
```

| Pros | Cons |
|------|------|
| Leverages existing 24/7 infrastructure | Provider coordination needed |
| No new network participants | Requires provider-to-provider protocol |
| Aligned incentives (providers need checkpoints) | Leader election complexity |
| Decentralized | Providers bear gas costs |
| Works for mobile/consumer apps | |
| Fallback mechanism for reliability | |

**Best suited for:** General-purpose decentralized storage where users should not need infrastructure.

---

## Comparison Matrix

| Criteria | Client-Initiated | Backend Server | Offchain Workers | Keeper Network | Challenge-Based | Provider-Initiated |
|----------|------------------|----------------|------------------|----------------|-----------------|-------------------|
| **Decentralization** | ✅ High | ❌ Low | ⚠️ Medium | ⚠️ Medium | ✅ High | ✅ High |
| **User Online Required** | ❌ Yes | ✅ No | ✅ No | ✅ No | ✅ No | ✅ No |
| **Additional Infrastructure** | ✅ None | ❌ Server | ✅ None | ❌ Keeper network | ✅ None | ✅ None |
| **Implementation Complexity** | ✅ Low | ✅ Low | ❌ High | ⚠️ Medium | ⚠️ Medium | ⚠️ Medium |
| **Mobile/Consumer Friendly** | ❌ No | ❌ No | ✅ Yes | ✅ Yes | ✅ Yes | ✅ Yes |
| **Reliability** | ❌ Low | ⚠️ Medium | ⚠️ Medium | ✅ High | ⚠️ Medium | ✅ High |
| **Cost to User** | ✅ Gas only | ❌ Server + Gas | ✅ Gas only | ⚠️ Fees + Gas | ✅ Gas only | ✅ Gas only |
| **Proactive Verification** | ✅ Yes | ✅ Yes | ✅ Yes | ✅ Yes | ❌ No | ✅ Yes |

**Legend:** ✅ Good | ⚠️ Medium | ❌ Poor

---

## Why Provider-Initiated Checkpoints?

After evaluating all options, provider-initiated checkpoints emerged as the best solution because:

1. **No new infrastructure**: Unlike keeper networks or backend servers, providers already exist and run 24/7
2. **Aligned incentives**: Providers NEED checkpoints to prove they're storing data and avoid challenges
3. **True decentralization**: No single point of failure, no trusted third parties
4. **Consumer friendly**: Mobile apps and regular users work without any infrastructure
5. **Fallback safety**: If one provider fails, others can submit
6. **Economic security**: Rewards and penalties ensure reliable operation

The main trade-off is implementation complexity, but this is a one-time cost that benefits all users of the system.

---

## Solution: Provider-Initiated Checkpoints

Providers themselves coordinate and submit checkpoints, removing the need for clients to be online.

---

## Architecture

### Current Flow (Client-Initiated)

```
┌──────────┐    1. Request signatures    ┌──────────────┐
│  Client  │ ───────────────────────────>│  Provider A  │
│  (User)  │<─────────────────────────── │              │
│          │    2. Return signature      └──────────────┘
│          │
│          │    1. Request signatures    ┌──────────────┐
│          │ ───────────────────────────>│  Provider B  │
│          │<─────────────────────────── │              │
│          │    2. Return signature      └──────────────┘
│          │
│          │    3. Submit checkpoint     ┌──────────────┐
│          │ ───────────────────────────>│  Blockchain  │
└──────────┘                             └──────────────┘

Problem: Client must be online to coordinate
```

### New Flow (Provider-Initiated)

```
┌──────────────┐    1. Broadcast MMR root    ┌──────────────┐
│  Provider A  │ ──────────────────────────> │  Provider B  │
│  (Leader)    │ <────────────────────────── │              │
│              │    2. Return signature      └──────────────┘
│              │
│              │    1. Broadcast MMR root    ┌──────────────┐
│              │ ──────────────────────────> │  Provider C  │
│              │ <────────────────────────── │              │
│              │    2. Return signature      └──────────────┘
│              │
│              │    3. Submit checkpoint     ┌──────────────┐
│              │ ──────────────────────────> │  Blockchain  │
└──────────────┘                             └──────────────┘

Solution: Providers coordinate among themselves
```

---

## Detailed Design

### 1. Leader Election

For each bucket and checkpoint window, one provider is elected as the "checkpoint leader":

```rust
/// Deterministic leader election based on the anchor (relay-chain) block.
/// `block_number` here — and every window/interval in this document — is an
/// anchor block read via `current_anchor_block()`, not the parachain height;
/// the `frame_system::block_number()` in the pseudocode stands in for that
/// anchor read.
fn elect_leader(
    bucket_id: BucketId,
    block_number: BlockNumber,
    providers: &[AccountId],
) -> AccountId {
    // Hash bucket_id + block_number for deterministic randomness
    let seed = blake2_256(&(bucket_id, block_number).encode());
    let index = u64::from_le_bytes(seed[0..8].try_into().unwrap()) as usize;
    providers[index % providers.len()].clone()
}
```

**Properties:**
- Deterministic: All providers compute the same leader
- Fair: Leadership rotates over time
- No coordination needed: Calculated independently

### 2. Checkpoint Window

Checkpoints are submitted in defined windows:

```rust
pub struct CheckpointConfig {
    /// Blocks between checkpoints (e.g., 100 blocks ≈ 10 minutes)
    pub checkpoint_interval: BlockNumber,

    /// Grace period for leader to submit (e.g., 50 blocks)
    pub leader_grace_period: BlockNumber,

    /// If leader fails, any provider can submit
    pub fallback_enabled: bool,
}
```

**Timeline:**
```
Block 0        Block 100       Block 150       Block 200
  │              │               │               │
  │──────────────│───────────────│───────────────│
  │   Window 1   │ Leader Grace  │   Window 2    │
  │              │    Period     │               │
  │              │               │               │
  └──────────────┴───────────────┴───────────────┘
                 ↑               ↑
           Leader submits   Fallback if
           checkpoint       leader failed
```

### 3. Provider Coordination Protocol

#### Step 1: Leader Announces Checkpoint

When checkpoint window opens, the leader broadcasts to all providers:

```rust
/// Message from leader to other providers
pub struct CheckpointProposal {
    pub bucket_id: BucketId,
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub window_block: BlockNumber,
    pub leader_signature: MultiSignature,
}
```

#### Step 2: Providers Verify and Sign

Each provider:
1. Verifies their local MMR matches the proposed root
2. Signs if matching
3. Returns signature to leader

```rust
/// Response from provider to leader
pub struct CheckpointVote {
    pub bucket_id: BucketId,
    pub mmr_root: H256,
    pub provider: AccountId,
    pub signature: MultiSignature,
}
```

#### Step 3: Leader Collects and Submits

Leader collects signatures until threshold met, then submits on-chain.

```rust
/// Extrinsic: checkpoint with provider submitter
pub fn provider_checkpoint(
    origin: OriginFor<T>,
    bucket_id: BucketId,
    mmr_root: H256,
    start_seq: u64,
    leaf_count: u64,
    signatures: BoundedVec<(T::AccountId, MultiSignature), T::MaxSignatures>,
) -> DispatchResult {
    let submitter = ensure_signed(origin)?;

    // Verify submitter is a provider for this bucket
    ensure!(
        Agreements::<T>::contains_key(bucket_id, &submitter),
        Error::<T>::NotBucketProvider
    );

    // Verify it's a valid checkpoint window
    let current_block = frame_system::Pallet::<T>::block_number();
    Self::verify_checkpoint_window(bucket_id, current_block)?;

    // Rest of checkpoint validation...
    Self::process_checkpoint(bucket_id, mmr_root, start_seq, leaf_count, signatures)
}
```

### 4. Economic Incentives

#### Checkpoint Reward Pool

Each bucket has a checkpoint reward funded by data owners:

```rust
pub struct BucketCheckpointConfig<Balance, BlockNumber> {
    /// Reward per successful checkpoint
    pub checkpoint_reward: Balance,

    /// Penalty for missing checkpoint window
    pub miss_penalty: Balance,

    /// Maximum blocks between checkpoints
    pub max_checkpoint_interval: BlockNumber,
}
```

#### Leader Reward

The submitting provider (leader) receives the checkpoint reward:

```rust
fn reward_checkpoint_submitter(
    bucket_id: BucketId,
    submitter: &AccountId,
    config: &BucketCheckpointConfig,
) {
    // Transfer reward from bucket's checkpoint pool
    let reward = config.checkpoint_reward;
    T::Currency::transfer(
        &Self::checkpoint_pool_account(bucket_id),
        submitter,
        reward,
        ExistenceRequirement::KeepAlive,
    )?;

    Self::deposit_event(Event::CheckpointRewardPaid {
        bucket_id,
        provider: submitter.clone(),
        amount: reward,
    });
}
```

#### Miss Penalty

If no checkpoint submitted in window, all providers are penalized:

```rust
fn penalize_missed_checkpoint(bucket_id: BucketId) {
    let config = BucketCheckpointConfigs::<T>::get(bucket_id);
    let providers = Self::get_bucket_providers(bucket_id);

    for provider in providers {
        // Slash from provider's stake
        let penalty = config.miss_penalty;
        Self::slash_stake(&provider, penalty);
    }

    Self::deposit_event(Event::CheckpointMissed { bucket_id });
}
```

### 5. Fallback Mechanism

If leader fails to submit, any provider can submit after grace period:

```rust
pub fn fallback_checkpoint(
    origin: OriginFor<T>,
    bucket_id: BucketId,
    // ... same params as provider_checkpoint
) -> DispatchResult {
    let submitter = ensure_signed(origin)?;

    let current_block = frame_system::Pallet::<T>::block_number();
    let window_start = Self::current_window_start(bucket_id);
    let grace_period = Self::checkpoint_grace_period();

    // Ensure we're past the leader grace period
    ensure!(
        current_block > window_start + grace_period,
        Error::<T>::LeaderGracePeriodActive
    );

    // Any provider can now submit
    Self::process_checkpoint(bucket_id, mmr_root, start_seq, leaf_count, signatures)
}
```

### 6. Disagreement Resolution

What if providers disagree on MMR root?

#### Scenario: Provider B has different data

```
Provider A (Leader): MMR root = 0xabc...
Provider B:          MMR root = 0xdef... (different!)
Provider C:          MMR root = 0xabc...
```

**Resolution:**
1. Provider B refuses to sign A's proposal
2. Leader still submits with A + C signatures (meets threshold)
3. Provider B can challenge if they believe their data is correct
4. Challenge mechanism resolves who has correct data

#### Scenario: Malicious Leader

```
Provider A (Leader): Proposes wrong MMR root
Provider B, C:       Refuse to sign
```

**Resolution:**
1. Leader can't get enough signatures
2. After grace period, B or C becomes fallback leader
3. Correct checkpoint gets submitted
4. Original leader gains nothing (no reward)

### 7. On-Chain Data Structures

```rust
/// Checkpoint window tracking
#[pallet::storage]
pub type LastCheckpointBlock<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    BucketId,
    BlockNumberFor<T>,
    ValueQuery,
>;

/// Checkpoint configuration per bucket
#[pallet::storage]
pub type BucketCheckpointConfigs<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    BucketId,
    BucketCheckpointConfig<BalanceOf<T>, BlockNumberFor<T>>,
    OptionQuery,
>;

/// Checkpoint reward pool balance
#[pallet::storage]
pub type CheckpointRewardPools<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    BucketId,
    BalanceOf<T>,
    ValueQuery,
>;
```

### 8. New Extrinsics

```rust
/// Configure checkpoint settings for a bucket
#[pallet::call_index(20)]
pub fn configure_checkpoints(
    origin: OriginFor<T>,
    bucket_id: BucketId,
    checkpoint_reward: BalanceOf<T>,
    max_interval: BlockNumberFor<T>,
) -> DispatchResult;

/// Fund the checkpoint reward pool
#[pallet::call_index(21)]
pub fn fund_checkpoint_pool(
    origin: OriginFor<T>,
    bucket_id: BucketId,
    amount: BalanceOf<T>,
) -> DispatchResult;

/// Provider-initiated checkpoint
#[pallet::call_index(22)]
pub fn provider_checkpoint(
    origin: OriginFor<T>,
    bucket_id: BucketId,
    mmr_root: H256,
    start_seq: u64,
    leaf_count: u64,
    signatures: BoundedVec<(T::AccountId, MultiSignature), T::MaxSignatures>,
) -> DispatchResult;
```

---

## Provider Node Changes

### 1. Checkpoint Scheduler

```rust
/// Runs on provider node
pub struct CheckpointScheduler {
    chain_client: SubstrateClient,
    buckets: HashMap<BucketId, BucketState>,
    peer_providers: HashMap<BucketId, Vec<ProviderPeer>>,
}

impl CheckpointScheduler {
    /// Called every block
    pub async fn tick(&mut self, current_block: BlockNumber) {
        for (bucket_id, state) in &self.buckets {
            if self.is_checkpoint_window(bucket_id, current_block) {
                if self.am_i_leader(bucket_id, current_block) {
                    self.initiate_checkpoint(bucket_id).await;
                }
            }
        }
    }

    async fn initiate_checkpoint(&self, bucket_id: &BucketId) {
        // 1. Get current MMR state
        let mmr_root = self.get_mmr_root(bucket_id);

        // 2. Create proposal
        let proposal = CheckpointProposal {
            bucket_id: *bucket_id,
            mmr_root,
            // ...
        };

        // 3. Request signatures from peers
        let signatures = self.collect_signatures(&proposal).await;

        // 4. Submit on-chain if threshold met
        if signatures.len() >= self.min_signatures(bucket_id) {
            self.submit_checkpoint(proposal, signatures).await;
        }
    }
}
```

### 2. Provider-to-Provider Communication

New HTTP endpoints on provider nodes:

```rust
/// POST /checkpoint/propose
/// Leader sends checkpoint proposal
async fn propose_checkpoint(
    State(state): State<AppState>,
    Json(proposal): Json<CheckpointProposal>,
) -> Result<Json<CheckpointVote>, ApiError> {
    // Verify we're a provider for this bucket
    // Verify our MMR matches
    // Sign and return vote
}

/// GET /checkpoint/status/:bucket_id
/// Query checkpoint status
async fn checkpoint_status(
    State(state): State<AppState>,
    Path(bucket_id): Path<BucketId>,
) -> Result<Json<CheckpointStatus>, ApiError> {
    // Return current checkpoint state
}
```

### 3. Provider Discovery

Providers need to know each other's endpoints:

```rust
/// Query on-chain for other providers
async fn discover_peers(&self, bucket_id: BucketId) -> Vec<ProviderPeer> {
    // Get all providers for this bucket from chain
    let agreements = self.chain_client.get_bucket_agreements(bucket_id).await;

    // Get multiaddr for each provider
    let mut peers = Vec::new();
    for (provider_id, _) in agreements {
        if let Some(info) = self.chain_client.get_provider_info(provider_id).await {
            peers.push(ProviderPeer {
                account: provider_id,
                multiaddr: info.multiaddr,
            });
        }
    }
    peers
}
```

---

## Implemented HTTP API (as shipped)

> ⚠️ **Drift from the proposal above.** These are the endpoints the provider
> node actually ships today (backing the autonomous checkpoint coordinator,
> `checkpoint_coordinator.rs`). They differ from the proposed
> `/checkpoint/propose` + `/checkpoint/status` in
> [Provider Node Changes](#2-provider-to-provider-communication) — reconcile the
> two during triage. Extracted from the Layer 0 implementation doc, which is
> review-gated and should not carry unratified design.

Primary providers exchange signed `CheckpointProposal`s over HTTP, and one of
them submits the `provider_checkpoint` extrinsic on-chain.

```
Sign a Checkpoint Proposal
──────────────────────────
POST /checkpoint/sign

Request:
{
  "bucket_id": 1234,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "window": 7
}

Response (200 OK):
{
  "signer": "5G...",                 // this provider's SS58 address
  "signature": "0x...",              // sr25519 over CheckpointProposal
  "agreed": true,                    // false if local state diverges
  "local_mmr_root": "0xfed..."       // included for divergence diagnostics
}

Note: If `agreed: false`, the signature is empty — the local view of the
bucket doesn't match the proposal. Callers compare `local_mmr_root` to
investigate divergence (e.g. one provider is behind).

Get Checkpoint Duty
───────────────────
GET /checkpoint/duty?bucket_id=1234

Response:
{
  "bucket_id": 1234,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "ready": true                      // false if leaf_count == 0
}

Trigger Checkpoint (operator-only)
──────────────────────────────────
POST /checkpoint/trigger?bucket_id=1234
Authorization: Web3Storage <...>     # Admin

Response:
{
  "bucket_id": 1234,
  "triggered": true,
  "message": "Checkpoint triggered for bucket 1234 with 42 leaves..."
}

Note: Sends a `ForceCheckpoint` command to the coordinator task. Requires the
provider to have been launched with `--enable-checkpoint-coordinator`,
otherwise returns 500. Mostly used in tests and manual recovery.
```

---

## Security Analysis

### Attack: Malicious Leader Submits Wrong Root

**Attack:** Leader submits checkpoint with wrong MMR root to frame other providers.

**Defense:**
- Checkpoint requires signatures from majority of providers
- Other providers verify MMR before signing
- Wrong root won't get enough signatures

### Attack: Leader Refuses to Submit

**Attack:** Leader intentionally doesn't submit to cause everyone to be penalized.

**Defense:**
- Fallback mechanism allows any provider to submit after grace period
- Leader gains nothing (loses potential reward)
- Repeated failures can be detected and reported

### Attack: Sybil Providers

**Attack:** Create many fake providers to control checkpoints.

**Defense:**
- Minimum stake requirement for providers
- Economic cost to attack
- Bucket owner selects which providers to use

### Attack: Provider Collusion

**Attack:** All providers collude to submit wrong checkpoint.

**Defense:**
- Data owner can still challenge
- Random challenges from chain
- Economic penalties for failed challenges

---

## Migration Plan

### Phase 1: Optional Provider Checkpoints
- Add new extrinsics alongside existing ones
- Bucket owners can opt-in
- Both systems work in parallel

### Phase 2: Default to Provider-Initiated
- New buckets default to provider-initiated
- Existing buckets can migrate
- Client-initiated still supported

### Phase 3: Deprecate Client-Initiated
- Remove client checkpoint extrinsic
- All checkpoints provider-initiated
- Simplify protocol

---

## Configuration Recommendations

### Small Bucket (Personal Use)
```
checkpoint_interval: 600 blocks (~1 hour)
checkpoint_reward: 1 token
miss_penalty: 10 tokens
min_providers: 1
```

### Medium Bucket (Small Business)
```
checkpoint_interval: 100 blocks (~10 minutes)
checkpoint_reward: 5 tokens
miss_penalty: 50 tokens
min_providers: 2
```

### Large Bucket (Enterprise)
```
checkpoint_interval: 50 blocks (~5 minutes)
checkpoint_reward: 10 tokens
miss_penalty: 100 tokens
min_providers: 3
```

---

## Summary

Provider-initiated checkpoints solve the "always online" problem by:

1. **Leveraging existing infrastructure**: Providers already run 24/7 servers
2. **Deterministic coordination**: Leader election without communication
3. **Economic incentives**: Rewards for submitting, penalties for missing
4. **Fallback safety**: Any provider can submit if leader fails
5. **Same security guarantees**: Multi-provider consensus prevents cheating

This enables true decentralization where mobile apps and regular consumers can use the storage system without running their own infrastructure.

---

# Re-implementation Guide (as-shipped specification)

The "Detailed Design" above is the **original proposal** and drifted from what
was actually built (different extrinsic set, no separate `fallback_checkpoint`,
different storage names, different HTTP endpoints). This section is the
**normative spec of the implementation as it shipped**, plus the defects to fix
if it is rebuilt. Read this first; use the
[Implementation Archive](#implementation-archive-code-removed-in-306) below for
the verbatim code and the removed-test behavior tables.

> **Precondition (from #306):** do not re-implement until the rationale is
> re-evaluated and reviewed — the "Why" above was never validated. A reviewed
> design must move back into `docs/design/` (review-gated) first.

## On-chain specification

**Clock.** Every duration below is in **anchor (relay-chain) blocks** read via
`Config::BlockNumberProvider` (`current_anchor_block()`), never the parachain
height — see the repo-wide `BlockNumberFor` = anchor-clock convention.

**Types** (`crates/primitives/storage/src/lib.rs`):

- `CheckpointWindowConfig<BlockNumber> { interval, grace_period, enabled }`
- `CheckpointProposal { version: u8 = 1, bucket_id, commitment: Commitment, window: u64 }`
  — the **signed payload is the SCALE encoding of this struct**; `window`
  provides replay protection across windows. Providers sign with the sr25519
  key registered as their on-chain `public_key`.

**Window / leader math** (pallet helpers, archived below):

```text
window(b)        = b / interval          (0 when interval == 0)
window_start(w)  = w * interval
grace_end(w)     = window_start(w) + grace_period   (inclusive)
leader_index     = u32::from_le(blake2_256(bucket_id.to_le_bytes(8) || window.to_le_bytes(8))[0..4]) % num_primary_providers
```

Config per bucket comes from `CheckpointConfigs`, falling back to the runtime
defaults with `enabled: true` — i.e. **the feature was on by default for every
bucket**; reconsider that default at re-evaluation time.

**Config constants** (both runtimes): `DefaultCheckpointInterval = 100` anchor
blocks (~10 min), `DefaultCheckpointGrace = 20` (~2 min),
`CheckpointReward = 1` token, `CheckpointMissPenalty = 0.5` token (reporter
bounty = 10% of the penalty actually slashed).

**Storage:** `CheckpointConfigs: Map<BucketId → CheckpointWindowConfig>`,
`LastCheckpointWindow: Map<BucketId → u64>` (`None` = never checkpointed),
`CheckpointRewards: DoubleMap<(AccountId, BucketId) → Balance>` —
**provider-first key order** so `complete_deregister` can drain via
`iter_prefix(&provider)` — and `CheckpointPool: Map<BucketId → Balance>`.

**Extrinsics** (shipped at call indices 32–36; re-check free indices when
re-adding):

1. `provider_checkpoint(bucket_id, commitment, window, signatures: BoundedVec<(AccountId, MultiSignature), MaxPrimaryProviders>)`
   — validation order: config `enabled`; `window == window(now)`;
   `window > LastCheckpointWindow` (else `CheckpointAlreadySubmitted`); bucket
   exists; `num_primary_providers > 0`; **during grace only the elected leader
   may submit, after grace any primary provider** (fallback is built in — there
   is no separate `fallback_checkpoint` extrinsic); frozen constraint
   `start_seq >= frozen_start_seq`; every signature verifies over
   `SCALE(CheckpointProposal)` and each signer is a primary provider (bitfield
   built into `primary_signers`); `signing_count >= bucket.min_providers`.
   Effects: update historical roots; replace `bucket.snapshot` with
   `commitment_nonce: 0` (**deliberate**: provider checkpoints sign
   `CheckpointProposal`, not `CommitmentPayload`, so `extend_checkpoint`'s
   late-signature flow does not apply to them); bump `total_snapshots`; set
   `LastCheckpointWindow = window`; if `CheckpointPool >= CheckpointReward`,
   decrement the pool and credit `CheckpointRewards[submitter][bucket]`
   (claimed later), else the checkpoint succeeds with reward 0. Emits
   `ProviderCheckpointSubmitted { bucket_id, mmr_root, window, leader: submitter, signers, reward }`.
2. `configure_checkpoint_window(bucket_id, interval, grace_period, enabled)` —
   bucket admin only; `enabled = false` disables the provider-initiated path
   (client-initiated `checkpoint` is unaffected).
3. `report_missed_checkpoint(bucket_id, window)` — permissionless. Requires
   `window < window(now)`, `window > LastCheckpointWindow`,
   `now > window_start(window + 1)` (grace fully elapsed). Slashes the
   **elected leader of that window** by `CheckpointMissPenalty` via
   `slash_reserved`, pays the reporter 10% of the actually-slashed amount,
   decrements `ProviderInfo.stake`, and sets `LastCheckpointWindow = window`
   to prevent re-reporting.
4. `claim_checkpoint_rewards(bucket_id)` — `take`s the caller's accumulated
   `CheckpointRewards` entry into free balance (`NoRewardsToClaim` when zero).
5. `fund_checkpoint_pool(bucket_id, amount)` — permissionless top-up.

**Interaction with provider exit:** `complete_deregister` must drain the
provider's pending `CheckpointRewards` (all buckets, via `iter_prefix`) into
free balance before unreserving stake; its benchmark seeded
`MaxBucketsPerMember` entries to price the drain.

## Known defects in the shipped implementation — fix these if rebuilding

1. **Funds handling is unsound.** `fund_checkpoint_pool` only
   `reserve`s on the funder and bumps a counter; the reserve is never
   transferred or released. Rewards (`claim_checkpoint_rewards`,
   the `complete_deregister` drain) and the reporter bounty are paid with
   `T::Currency::deposit_creating`, i.e. **minted from nothing** while the
   funder's reserve stays locked forever. Re-implementation should hold the
   pool in a real (pallet sub-)account and pay by transfer.
2. **Miss penalty diverges from the design.** The proposal penalized all
   providers of the bucket; shipped code slashes only the elected leader.
   Also `report_missed_checkpoint` sets `LastCheckpointWindow = window`, which
   silently blocks reporting *older* missed windows and blocks a late (still
   valid at head) submission for that window.
3. **Coordinator was a skeleton.** `get_active_checkpoint_duties()` returned
   `[]` (the chain query for "buckets where I am primary and checkpoints are
   enabled" was a TODO), so the poll loop never did anything —
   only the operator-triggered `/checkpoint/trigger` path worked;
   `is_leader` was hard-coded `true` for forced duties (no node-side leader
   election); `peer_endpoints` was always empty (peer discovery
   unimplemented, so multi-provider signature collection never ran);
   the signature threshold was hard-coded `min_required = 1` instead of
   reading `bucket.min_providers`; `fetch_checkpoint_config` fell back to
   literal `(100, 20)` instead of the runtime constants; submission waited
   for finalization (slow for a 6 s poll loop).
4. **Dead errors.** `CheckpointWindowNotStarted`, `NoMissedCheckpoint` and
   `InsufficientCheckpointPool` were declared but never returned.
5. **Weights.** `provider_checkpoint` was `Linear<1, 5>` over signature count
   (`MaxPrimaryProviders = 5`); re-benchmark everything, including
   `complete_deregister` (its current weight still prices the removed drain).

## Off-chain specification (provider node)

Module `provider-node/src/checkpoint_coordinator.rs` (full code archived
below): a tokio background service created in `command.rs` behind
`--enable-checkpoint-coordinator` / `ENABLE_CHECKPOINT_COORDINATOR` (requires
`--keyfile` + reachable chain), polling every 6 s, controlled via
`CoordinatorCommand::{Stop, Pause, Resume, ForceCheckpoint(bucket)}` over an
mpsc channel that `ProviderState.checkpoint_cmd_tx` exposes to the HTTP layer.
Flow per duty: build `CheckpointProposal` from local storage state → sign
locally → `POST /checkpoint/sign` to each peer → submit `provider_checkpoint`
through `CheckpointChainClient` (implemented on `SubxtChainClient` with a
dynamic tx). HTTP surface (wire formats in
[Implemented HTTP API (as shipped)](#implemented-http-api-as-shipped)):

- `POST /checkpoint/sign` — unauthenticated peer endpoint; verifies the
  proposal against local `(mmr_root, start_seq, leaf_count)`, returns
  `{signer, signature, agreed, local_mmr_root}`; empty signature +
  `agreed: false` on divergence; 503 `signing_unavailable` without a key.
- `GET /checkpoint/duty?bucket_id=` — local readiness
  (`ready = leaf_count > 0`) + current commitment fields.
- `POST /checkpoint/trigger?bucket_id=` — **Writer-authenticated**; sends
  `ForceCheckpoint` to the coordinator; 500 if the coordinator isn't running.

## Client surface to restore

- `packages/layer0`: tx wrappers `configureCheckpointWindow`,
  `fundCheckpointPool`, `submitProviderCheckpoint`, `claimCheckpointRewards`,
  `reportMissedCheckpoint` (each `submitTx` + `requireOneEvent` on its event);
  HTTP helpers `fetchCheckpointDuty`, `signCheckpointProposal`.
- `packages/layer1`: `getCheckpointDuty` / `triggerCheckpoint` +
  `CheckpointDuty` type (consumed by drive-ui/s3-ui checkpoint panels).
- UIs: provider dashboard bucket detail (config / pool / pending reward /
  overdue flag, where `overdue = expected_window > last_window + 1` on the
  anchor clock); drive-ui + s3-ui duty display + trigger button.
- Example `examples/papi/checkpoint-missed.ts` (+ `just papi-checkpoint-missed`)
  exercising the miss-report slashing path end-to-end.

## Re-implementation order

1. `crates/primitives/storage`: re-add the two types.
2. `crates/pallets/storage-provider`: Config items → storage → events/errors →
   helpers (`impls/checkpoints.rs`) → the five extrinsics → the
   `complete_deregister` drain → `mock.rs` config (10/5/10/50 test values) →
   unit tests (behavior tables below) → benchmarks.
3. Mocks in `drive-registry` / `s3-registry` (`100/20/1e12/5e11`) and both
   runtimes' `storage.rs` (constants + Config wiring; paseo uses
   `pub storage`).
4. Weights: `frame-omni-bencher v1 benchmark pallet` (or `/cmd bench`) for
   pallet + both runtimes.
5. Regenerate chain artifacts: `just subxt-codegen` (paseo bindings +
   `.scale`) and `packages/papi` metadata (`papi update parachain` against a
   running chain; if regenerating descriptors from a replaced `.scale`, delete
   `.papi/descriptors/generated.json` first or `papi generate` no-ops).
6. Provider node: module + endpoints + CLI flag + `command.rs` wiring +
   `ProviderState.checkpoint_cmd_tx` + `CheckpointChainClient` impl; restore
   coordinator/API/auth tests; re-add the flag to `just start-provider` and
   the four CI provider launches in
   `.github/workflows/integration-tests.yml`.
7. SDK + example + e2e (former tests 5.4/5.5/5.7 in
   `examples/papi/e2e/05-checkpoint-and-challenges.ts`) + UIs.
8. Docs: extrinsic entries + workflow + error-table rows in
   `EXTRINSICS_REFERENCE.md`, runtime params in `CLAUDE.md`, and move the
   reviewed design into `docs/design/`.

---

# Implementation Archive (code removed in #306)

Everything below is the **actual shipped implementation** that was removed when
#306 resolved to "remove entirely". It is archived verbatim so the feature can
be re-implemented later without re-deriving the details. The last commit that
contains the live code is the parent of the removal commit (see
`git log -- provider-node/src/checkpoint_coordinator.rs` for the history).

Removal inventory:

| Layer | Removed |
| --- | --- |
| Primitives (`crates/primitives/storage/src/lib.rs`) | `CheckpointWindowConfig`, `CheckpointProposal` |
| Pallet config | `DefaultCheckpointInterval`, `DefaultCheckpointGrace`, `CheckpointReward`, `CheckpointMissPenalty` |
| Pallet storage | `CheckpointConfigs`, `LastCheckpointWindow`, `CheckpointRewards`, `CheckpointPool` |
| Pallet events | `ProviderCheckpointSubmitted`, `CheckpointConfigUpdated`, `CheckpointMissPenalized`, `CheckpointRewardClaimed`, `CheckpointPoolFunded` |
| Pallet errors | `ProviderCheckpointsDisabled`, `NotCheckpointLeader`, `CheckpointWindowNotStarted`, `CheckpointAlreadySubmitted`, `InvalidCheckpointWindow`, `InsufficientCheckpointPool`, `NoMissedCheckpoint`, `WithinGracePeriod`, `NoRewardsToClaim` |
| Extrinsics (call indices 32–36) | `provider_checkpoint`, `configure_checkpoint_window`, `report_missed_checkpoint`, `claim_checkpoint_rewards`, `fund_checkpoint_pool` |
| Pallet helpers | `calculate_window`, `window_start_block`, `calculate_leader_index`, `get_checkpoint_config`, `is_within_grace_period`; `complete_deregister`'s pending-reward drain |
| Provider node | `checkpoint_coordinator.rs` (whole module), `/checkpoint/sign` + `/checkpoint/duty` + `/checkpoint/trigger` endpoints, `--enable-checkpoint-coordinator` CLI flag, `CheckpointChainClient` impl in `subxt_client.rs`, `ProviderState::checkpoint_cmd_tx` |
| JS/TS SDK | `configureCheckpointWindow`, `fundCheckpointPool`, `submitProviderCheckpoint`, `claimCheckpointRewards`, `reportMissedCheckpoint`, `fetchCheckpointDuty`, `signCheckpointProposal`, layer-1 `getCheckpointDuty`/`triggerCheckpoint` |
| UIs | provider dashboard checkpoint config/pool/reward/overdue display; drive-ui + s3-ui checkpoint duty/trigger panels |
| Examples / tooling | `examples/papi/checkpoint-missed.ts`, e2e tests 5.4/5.5/5.7, `just papi-checkpoint-missed`, coordinator flag in `just start-provider` + CI workflows |

> **Not removed:** the client-initiated `checkpoint` / `extend_checkpoint` /
> `challenge_checkpoint` extrinsics, the `/checkpoint-signature` provider
> endpoint, and everything reading `bucket.snapshot` — those are a separate,
> canonical mechanism.

If this is ever re-implemented, note the follow-ups that were *not* archived
as code: regenerate weights via benchmarks (`provider_checkpoint` had a
`Linear<1, 5>` signature dimension), regenerate the subxt bindings
(`just subxt-codegen`) and the PAPI metadata (`packages/papi` →
`papi update parachain`), and re-add coverage for the coordinator in
`scripts/coverage.sh`.

## Shared primitives (`crates/primitives/storage/src/lib.rs`)

```rust
/// Configuration for provider-initiated checkpoints.
///
/// Providers can autonomously coordinate checkpoints without requiring
/// the client to be online. Uses deterministic leader election and
/// checkpoint windows with grace periods.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CheckpointWindowConfig<BlockNumber> {
    /// Blocks between checkpoints (e.g., 100 blocks = ~10 minutes)
    pub interval: BlockNumber,
    /// Grace period for leader before fallback (e.g., 20 blocks = ~2 minutes)
    pub grace_period: BlockNumber,
    /// Whether provider-initiated checkpoints are enabled for this bucket
    pub enabled: bool,
}

/// Proposal for provider-initiated checkpoint (signed by providers).
///
/// This is the payload that providers sign to agree on a checkpoint.
/// The window number prevents cross-window replay attacks.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CheckpointProposal {
    /// Protocol version for future compatibility
    pub version: u8,
    /// Reference to on-chain bucket
    pub bucket_id: BucketId,
    /// MMR commitment being proposed (root + covered leaf range).
    pub commitment: Commitment,
    /// Window number this proposal is for (prevents replay)
    pub window: u64,
}

impl CheckpointProposal {
    /// Current protocol version
    pub const CURRENT_VERSION: u8 = 1;

    /// Create a new checkpoint proposal
    pub fn new(
        bucket_id: BucketId,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        window: u64,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bucket_id,
            commitment: Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            },
            window,
        }
    }

    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.commitment.range_end()
    }

    /// Check if a sequence number is within this proposal's range
    pub fn contains_seq(&self, seq: u64) -> bool {
        self.commitment.contains_seq(seq)
    }
}
```

## Pallet: `pallet-storage-provider`

### Config constants

```rust
        /// Default interval between provider-initiated checkpoints (e.g., 100
        /// relay chain blocks).
        #[pallet::constant]
        type DefaultCheckpointInterval: Get<BlockNumberFor<Self>>;

        /// Default grace period for checkpoint leader (e.g., 20 relay chain
        /// blocks).
        #[pallet::constant]
        type DefaultCheckpointGrace: Get<BlockNumberFor<Self>>;

        /// Reward paid to provider for submitting a checkpoint.
        #[pallet::constant]
        type CheckpointReward: Get<BalanceOf<Self>>;

        /// Penalty for missing a checkpoint window (slashed from provider stake).
        #[pallet::constant]
        type CheckpointMissPenalty: Get<BalanceOf<Self>>;
```

### Storage items

```rust
    // ─────────────────────────────────────────────────────────────────────────
    // Provider-Initiated Checkpoint Storage
    // ─────────────────────────────────────────────────────────────────────────

    /// Checkpoint window configuration per bucket.
    /// When None, bucket uses runtime defaults.
    #[pallet::storage]
    pub type CheckpointConfigs<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        BucketId,
        storage_primitives::CheckpointWindowConfig<BlockNumberFor<T>>,
    >;

    /// Last successful checkpoint window per bucket.
    /// `None` means no checkpoint has been submitted yet.
    #[pallet::storage]
    pub type LastCheckpointWindow<T: Config> =
        StorageMap<_, Blake2_128Concat, BucketId, u64, OptionQuery>;

    /// Pending checkpoint rewards per (provider, bucket).
    /// Accumulates rewards for providers who submit or sign checkpoints.
    /// Provider-first key order enables `iter_prefix(&provider)` so a
    /// provider's pending rewards can be drained on deregistration without
    /// scanning every bucket.
    #[pallet::storage]
    pub type CheckpointRewards<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        Blake2_128Concat,
        BucketId,
        BalanceOf<T>,
        ValueQuery,
    >;

    /// Checkpoint pool balance per bucket.
    /// Funded by clients to pay for provider-initiated checkpoints.
    #[pallet::storage]
    pub type CheckpointPool<T: Config> =
        StorageMap<_, Blake2_128Concat, BucketId, BalanceOf<T>, ValueQuery>;
```

### Events

```rust
        // Provider-initiated checkpoint events
        ProviderCheckpointSubmitted {
            bucket_id: BucketId,
            mmr_root: H256,
            window: u64,
            leader: T::AccountId,
            signers: Vec<T::AccountId>,
            reward: BalanceOf<T>,
        },
        CheckpointConfigUpdated {
            bucket_id: BucketId,
            interval: BlockNumberFor<T>,
            grace_period: BlockNumberFor<T>,
            enabled: bool,
        },
        CheckpointMissPenalized {
            bucket_id: BucketId,
            provider: T::AccountId,
            window: u64,
            penalty: BalanceOf<T>,
        },
        CheckpointRewardClaimed {
            bucket_id: BucketId,
            provider: T::AccountId,
            amount: BalanceOf<T>,
        },
        CheckpointPoolFunded {
            bucket_id: BucketId,
            funder: T::AccountId,
            amount: BalanceOf<T>,
        },
```

### Errors

```rust
        // Provider-initiated checkpoint errors
        /// Provider-initiated checkpoints are disabled for this bucket.
        ProviderCheckpointsDisabled,
        /// Caller is not the designated checkpoint leader for this window.
        NotCheckpointLeader,
        /// Checkpoint window has not started yet.
        CheckpointWindowNotStarted,
        /// Checkpoint has already been submitted for this window.
        CheckpointAlreadySubmitted,
        /// Invalid checkpoint window number.
        InvalidCheckpointWindow,
        /// Insufficient funds in checkpoint pool to pay reward.
        InsufficientCheckpointPool,
        /// No missed checkpoint to report.
        NoMissedCheckpoint,
        /// Cannot report miss while still within grace period.
        WithinGracePeriod,
        /// No rewards to claim.
        NoRewardsToClaim,
```

### Extrinsics (call indices 32–36)

`provider_checkpoint` verified sr25519 signatures over the SCALE-encoded
`CheckpointProposal`, enforced the leader/grace/fallback rules, updated the
bucket snapshot exactly like the client-initiated path (but with
`commitment_nonce: 0` — see the inline comment), and credited the reward.

```rust
        // ─────────────────────────────────────────────────────────────────────
        // Provider-Initiated Checkpoints
        // ─────────────────────────────────────────────────────────────────────

        /// Submit a provider-initiated checkpoint.
        ///
        /// Providers autonomously coordinate checkpoints without requiring
        /// clients to be online. Uses deterministic leader election with
        /// fallback to any primary provider after grace period.
        ///
        /// Parameters:
        /// - `bucket_id`: The bucket to checkpoint
        /// - `mmr_root`: MMR root that providers agreed on
        /// - `start_seq`: Starting sequence number
        /// - `leaf_count`: Number of leaves in the MMR
        /// - `window`: Checkpoint window number (prevents replay)
        /// - `signatures`: Provider signatures over the checkpoint proposal
        #[pallet::call_index(32)]
        #[pallet::weight(T::WeightInfo::provider_checkpoint(signatures.len() as u32))]
        pub fn provider_checkpoint(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            commitment: Commitment,
            window: u64,
            signatures: BoundedVec<
                (T::AccountId, sp_runtime::MultiSignature),
                T::MaxPrimaryProviders,
            >,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let Commitment {
                mmr_root,
                start_seq,
                leaf_count,
            } = commitment;

            // Get checkpoint config
            let config = Self::get_checkpoint_config(bucket_id);
            ensure!(config.enabled, Error::<T>::ProviderCheckpointsDisabled);

            // Get current block and calculate current window
            let anchor_block = Self::current_anchor_block();
            let current_window = Self::calculate_window(anchor_block, config.interval);

            // Validate window
            ensure!(
                window == current_window,
                Error::<T>::InvalidCheckpointWindow
            );

            // Check if already submitted for this window
            if let Some(last_window) = LastCheckpointWindow::<T>::get(bucket_id) {
                ensure!(window > last_window, Error::<T>::CheckpointAlreadySubmitted);
            }

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                let num_providers = bucket.primary_providers.len() as u32;
                ensure!(num_providers > 0, Error::<T>::MinProvidersNotMet);

                // Calculate expected leader
                let leader_idx = Self::calculate_leader_index(bucket_id, window, num_providers);
                let expected_leader = bucket
                    .primary_providers
                    .get(leader_idx as usize)
                    .ok_or(Error::<T>::ProviderNotInSnapshot)?;

                // Check caller authorization
                let within_grace = Self::is_within_grace_period(anchor_block, window, &config);
                if within_grace {
                    // Only leader can submit during grace period
                    ensure!(&who == expected_leader, Error::<T>::NotCheckpointLeader);
                } else {
                    // After grace period, any primary provider can submit (fallback)
                    ensure!(
                        bucket.primary_providers.contains(&who),
                        Error::<T>::ProviderNotInSnapshot
                    );
                }

                // Check frozen constraint
                if let Some(frozen_start) = bucket.frozen_start_seq {
                    ensure!(
                        start_seq >= frozen_start,
                        Error::<T>::SnapshotViolatesFrozen
                    );
                }

                // Verify signatures using CheckpointProposal
                let proposal = storage_primitives::CheckpointProposal::new(
                    bucket_id, mmr_root, start_seq, leaf_count, window,
                );
                let encoded_proposal = proposal.encode();

                // Create bitfield using Vec<u8>
                let num_bytes = (num_providers as usize).div_ceil(8);
                let mut primary_signers = vec![0u8; num_bytes];
                let mut signing_count = 0usize;
                let mut signing_providers = Vec::new();

                for (signer, signature) in signatures.iter() {
                    // Find signer in primary_providers
                    let idx = bucket
                        .primary_providers
                        .iter()
                        .position(|p| p == signer)
                        .ok_or(Error::<T>::ProviderNotInSnapshot)?;

                    // Verify the signature
                    Self::verify_signature(signature, &encoded_proposal, signer)?;

                    // Set bit at position idx
                    let byte_idx = idx / 8;
                    let bit_idx = idx % 8;
                    primary_signers[byte_idx] |= 1 << bit_idx;
                    signing_count += 1;
                    signing_providers.push(signer.clone());
                }

                // Check min_providers threshold
                ensure!(
                    signing_count >= bucket.min_providers as usize,
                    Error::<T>::InsufficientSignatures
                );

                // Update historical roots
                Self::update_historical_roots(bucket, anchor_block, mmr_root);

                // Update bucket snapshot.
                //
                // `commitment_nonce` is only meaningful for snapshots produced
                // by the client-initiated `checkpoint` extrinsic (which signs
                // over `CommitmentPayload`). Provider-initiated checkpoints
                // sign over `CheckpointProposal::window` instead, so
                // `extend_checkpoint` (which expects `CommitmentPayload`-shaped
                // late signatures) is not applicable here — leave the nonce
                // at zero rather than smuggling in `window` and confusing the
                // two schemes.
                bucket.snapshot = Some(BucketSnapshot {
                    commitment,
                    checkpoint_block: anchor_block,
                    primary_signers,
                    commitment_nonce: 0,
                });
                bucket.total_snapshots = bucket.total_snapshots.saturating_add(1);

                // Update last checkpoint window
                LastCheckpointWindow::<T>::insert(bucket_id, window);

                // Pay reward from pool to submitter
                let reward = T::CheckpointReward::get();
                let pool_balance = CheckpointPool::<T>::get(bucket_id);

                let actual_reward = if pool_balance >= reward {
                    CheckpointPool::<T>::mutate(bucket_id, |balance| {
                        *balance = balance.saturating_sub(reward);
                    });
                    // Unreserve from pool and transfer to submitter
                    // Note: Pool funds are reserved by funder, we pay submitter directly
                    CheckpointRewards::<T>::mutate(&who, bucket_id, |pending| {
                        *pending = pending.saturating_add(reward);
                    });
                    reward
                } else {
                    // Pool empty - checkpoint still valid but no reward
                    Zero::zero()
                };

                Self::deposit_event(Event::ProviderCheckpointSubmitted {
                    bucket_id,
                    mmr_root,
                    window,
                    leader: who.clone(),
                    signers: signing_providers,
                    reward: actual_reward,
                });

                Ok(())
            })
        }

        /// Configure checkpoint window settings for a bucket.
        ///
        /// Only bucket admin can configure. Setting enabled=false disables
        /// provider-initiated checkpoints (client-initiated still work).
        #[pallet::call_index(33)]
        #[pallet::weight(T::WeightInfo::configure_checkpoint_window())]
        pub fn configure_checkpoint_window(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            interval: BlockNumberFor<T>,
            grace_period: BlockNumberFor<T>,
            enabled: bool,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            Self::ensure_admin(&who, &bucket)?;

            let config = storage_primitives::CheckpointWindowConfig {
                interval,
                grace_period,
                enabled,
            };

            CheckpointConfigs::<T>::insert(bucket_id, config);

            Self::deposit_event(Event::CheckpointConfigUpdated {
                bucket_id,
                interval,
                grace_period,
                enabled,
            });

            Ok(())
        }

        /// Report a missed checkpoint window and penalize the leader.
        ///
        /// Can only be called after the checkpoint window has fully passed
        /// (beyond grace period) and no checkpoint was submitted.
        /// Reporter receives a portion of the penalty.
        #[pallet::call_index(34)]
        #[pallet::weight(T::WeightInfo::report_missed_checkpoint())]
        pub fn report_missed_checkpoint(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            window: u64,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            let config = Self::get_checkpoint_config(bucket_id);

            ensure!(config.enabled, Error::<T>::ProviderCheckpointsDisabled);

            // Get current window
            let anchor_block = Self::current_anchor_block();
            let current_window = Self::calculate_window(anchor_block, config.interval);

            // Can only report past windows
            ensure!(window < current_window, Error::<T>::InvalidCheckpointWindow);

            // Check that window wasn't submitted
            if let Some(last_window) = LastCheckpointWindow::<T>::get(bucket_id) {
                ensure!(window > last_window, Error::<T>::CheckpointAlreadySubmitted);
            }

            // Ensure we're past the grace period of the reported window
            let window_end = Self::window_start_block(window.saturating_add(1), config.interval);
            ensure!(anchor_block > window_end, Error::<T>::WithinGracePeriod);

            // Calculate leader for the missed window
            let num_providers = bucket.primary_providers.len() as u32;
            ensure!(num_providers > 0, Error::<T>::MinProvidersNotMet);

            let leader_idx = Self::calculate_leader_index(bucket_id, window, num_providers);
            let leader = bucket
                .primary_providers
                .get(leader_idx as usize)
                .ok_or(Error::<T>::ProviderNotInSnapshot)?
                .clone();

            // Apply penalty to leader's stake
            let penalty = T::CheckpointMissPenalty::get();
            let (_, remaining) = T::Currency::slash_reserved(&leader, penalty);
            let actual_penalty = penalty.saturating_sub(remaining);

            // Give reporter 10% of penalty
            let reporter_reward = actual_penalty / 10u32.into();
            if !reporter_reward.is_zero() {
                let _ = T::Currency::deposit_creating(&who, reporter_reward);
            }

            // Update provider stats
            Providers::<T>::mutate(&leader, |maybe_provider| {
                if let Some(provider) = maybe_provider {
                    provider.stake = provider.stake.saturating_sub(actual_penalty);
                }
            });

            // Update last checkpoint window to prevent re-reporting
            LastCheckpointWindow::<T>::insert(bucket_id, window);

            Self::deposit_event(Event::CheckpointMissPenalized {
                bucket_id,
                provider: leader,
                window,
                penalty: actual_penalty,
            });

            Ok(())
        }

        /// Claim accumulated checkpoint rewards.
        ///
        /// Providers accumulate rewards for submitting checkpoints.
        /// This transfers accumulated rewards to the provider.
        #[pallet::call_index(35)]
        #[pallet::weight(T::WeightInfo::claim_checkpoint_rewards())]
        pub fn claim_checkpoint_rewards(
            origin: OriginFor<T>,
            bucket_id: BucketId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let rewards = CheckpointRewards::<T>::take(&who, bucket_id);
            ensure!(!rewards.is_zero(), Error::<T>::NoRewardsToClaim);

            // Transfer rewards to provider
            let _ = T::Currency::deposit_creating(&who, rewards);

            Self::deposit_event(Event::CheckpointRewardClaimed {
                bucket_id,
                provider: who,
                amount: rewards,
            });

            Ok(())
        }

        /// Fund the checkpoint reward pool for a bucket.
        ///
        /// Anyone can fund the pool. Funds are used to reward providers
        /// for submitting checkpoints.
        #[pallet::call_index(36)]
        #[pallet::weight(T::WeightInfo::fund_checkpoint_pool())]
        pub fn fund_checkpoint_pool(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            amount: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(
                Buckets::<T>::contains_key(bucket_id),
                Error::<T>::BucketNotFound
            );

            // Reserve funds from funder
            T::Currency::reserve(&who, amount)?;

            // Add to pool
            CheckpointPool::<T>::mutate(bucket_id, |balance| {
                *balance = balance.saturating_add(amount);
            });

            Self::deposit_event(Event::CheckpointPoolFunded {
                bucket_id,
                funder: who,
                amount,
            });

            Ok(())
        }
```

### Window / leader helpers (`impls/checkpoints.rs`)

```rust
impl<T: Config> Pallet<T> {
    /// Calculate the checkpoint window number for a given block.
    ///
    /// Window 0 starts at block 0, window 1 at block `interval`, etc.
    pub(crate) fn calculate_window(block: BlockNumberFor<T>, interval: BlockNumberFor<T>) -> u64 {
        if interval.is_zero() {
            return 0;
        }
        let block_num: u64 = block.saturated_into();
        let interval_num: u64 = interval.saturated_into();
        block_num / interval_num
    }

    /// Calculate the start block for a given checkpoint window.
    pub(crate) fn window_start_block(
        window: u64,
        interval: BlockNumberFor<T>,
    ) -> BlockNumberFor<T> {
        let interval_num: u64 = interval.saturated_into();
        let start: u64 = window.saturating_mul(interval_num);
        start.saturated_into()
    }

    /// Calculate the leader index for a given bucket and window.
    ///
    /// Uses deterministic selection: blake2_256(bucket_id || window) % num_providers.
    /// This ensures all providers can independently calculate who the leader is.
    pub(crate) fn calculate_leader_index(
        bucket_id: BucketId,
        window: u64,
        num_providers: u32,
    ) -> u32 {
        if num_providers == 0 {
            return 0;
        }
        // Create deterministic seed from bucket_id and window
        let mut data = [0u8; 16];
        data[..8].copy_from_slice(&bucket_id.to_le_bytes());
        data[8..].copy_from_slice(&window.to_le_bytes());
        let hash = sp_io::hashing::blake2_256(&data);
        // Take first 4 bytes as u32 and mod by num_providers
        let seed = u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]);
        seed % num_providers
    }

    /// Get the checkpoint config for a bucket, falling back to defaults.
    pub(crate) fn get_checkpoint_config(
        bucket_id: BucketId,
    ) -> storage_primitives::CheckpointWindowConfig<BlockNumberFor<T>> {
        CheckpointConfigs::<T>::get(bucket_id).unwrap_or_else(|| {
            storage_primitives::CheckpointWindowConfig {
                interval: T::DefaultCheckpointInterval::get(),
                grace_period: T::DefaultCheckpointGrace::get(),
                enabled: true, // Enabled by default
            }
        })
    }

    /// Check if the anchor block is within the grace period for a window.
    pub(crate) fn is_within_grace_period(
        anchor_block: BlockNumberFor<T>,
        window: u64,
        config: &storage_primitives::CheckpointWindowConfig<BlockNumberFor<T>>,
    ) -> bool {
        let window_start = Self::window_start_block(window, config.interval);
        let grace_end = window_start.saturating_add(config.grace_period);
        anchor_block <= grace_end
    }
}
```

### `complete_deregister` reward drain

`complete_deregister` drained pending `CheckpointRewards` (provider-first key
order made `iter_prefix(&provider)` possible) before unreserving stake:

```rust
            // Drain pending checkpoint rewards (provider-keyed thanks to the
            // (AccountId, BucketId) layout of CheckpointRewards).
            let mut total_rewards: BalanceOf<T> = Zero::zero();
            let drained: Vec<BucketId> = CheckpointRewards::<T>::iter_prefix(&who)
                .map(|(bucket_id, _)| bucket_id)
                .collect();
            for bucket_id in drained {
                let amount = CheckpointRewards::<T>::take(&who, bucket_id);
                total_rewards = total_rewards.saturating_add(amount);
            }
            if !total_rewards.is_zero() {
                let _ = T::Currency::deposit_creating(&who, total_rewards);
            }
```

### Mock / test / benchmark configuration

- pallet `mock.rs`: `DefaultCheckpointInterval = 10`, `DefaultCheckpointGrace = 5`,
  `CheckpointReward = 10`, `CheckpointMissPenalty = 50`
- `drive-registry` / `s3-registry` mocks: `100 / 20 / 1_000_000_000_000 / 500_000_000_000`
- Benchmarks removed: `provider_checkpoint(s: Linear<1, 5>)` (s sr25519-signing
  primaries via `add_primary_to_bucket` + `register_sr25519_key`, funded pool,
  past-grace fallback submission), `configure_checkpoint_window`,
  `report_missed_checkpoint`, `claim_checkpoint_rewards`, `fund_checkpoint_pool`;
  `complete_deregister`'s benchmark also seeded `MaxBucketsPerMember` reward
  entries to measure the drain (now unnecessary — re-benchmark it).

### Unit tests removed (`tests/checkpoint.rs`, `tests/provider.rs`)

Helpers: `sign_checkpoint_proposal(pair, signer, bucket, root, start_seq,
leaf_count, window)` signed the SCALE-encoded proposal;
`setup_agreement_with_keypair` returned `(bucket_id, sr25519::Pair)` with the
pair stamped into the provider's on-chain `public_key`.

| Test | Behavior pinned |
| --- | --- |
| `configure_checkpoint_window_works` | admin sets `{interval, grace_period, enabled}`, readable from `CheckpointConfigs` |
| `configure_checkpoint_window_fails_not_admin` | `NotBucketAdmin` |
| `configure_checkpoint_window_fails_no_bucket` | `BucketNotFound` |
| `fund_checkpoint_pool_works` | reserves funder balance, `CheckpointPool += amount` |
| `fund_checkpoint_pool_fails_no_bucket` | `BucketNotFound` |
| `claim_checkpoint_rewards_works` | pays out and zeroes `CheckpointRewards` |
| `claim_checkpoint_rewards_fails_no_rewards` | `NoRewardsToClaim` |
| `report_missed_checkpoint_fails_within_grace` | reporting the current window → `InvalidCheckpointWindow` |
| `report_missed_checkpoint_works` | past window end: leader slashed, reporter gets 10% of actual penalty |
| `report_missed_checkpoint_emits_event` | `CheckpointMissPenalized { bucket, provider, window }` |
| `report_missed_checkpoint_fails_already_submitted` | submitted window can't be reported |
| `provider_checkpoint_fails_disabled` | `enabled = false` → `ProviderCheckpointsDisabled` |
| `provider_checkpoint_fails_wrong_window` | non-current window → `InvalidCheckpointWindow` |
| `provider_checkpoint_leader_within_grace_period` | leader submits during grace; snapshot + `LastCheckpointWindow` updated |
| `provider_checkpoint_non_leader_rejected_during_grace` | replicates leader election, non-leader → `NotCheckpointLeader` |
| `provider_checkpoint_fallback_after_grace` | past grace any primary may submit |
| `provider_checkpoint_already_submitted` | same window twice → `CheckpointAlreadySubmitted` |
| `provider_checkpoint_frozen_constraint` | `start_seq < frozen_start_seq` → `SnapshotViolatesFrozen` |
| `provider_checkpoint_reward_from_pool` | pool decremented by `CheckpointReward`, credit to `CheckpointRewards` |
| `provider_checkpoint_no_reward_empty_pool` | empty pool: checkpoint still valid, reward 0 |
| `provider_checkpoint_emits_event` | `ProviderCheckpointSubmitted { window, leader, signers, reward }` |
| `complete_deregister_drains_checkpoint_rewards` (provider.rs) | drain pays only the exiting provider's entries |

## Runtime wiring (both runtimes)

`runtimes/web3-storage-local/src/storage.rs` (`pub const`) and
`runtimes/web3-storage-paseo/src/storage.rs` (`pub storage`):

```rust
pub const DefaultCheckpointInterval: BlockNumber = 100; // relay blocks (~10 min)
pub const DefaultCheckpointGrace: BlockNumber = 20; // relay blocks (~2 min)
pub const CheckpointReward: Balance = 1_000_000_000_000; // 1 token
pub const CheckpointMissPenalty: Balance = 500_000_000_000; // 0.5 token
```

plus the four `type X = X;` lines in each runtime's
`impl pallet_storage_provider::Config`.

## Provider node

### `provider-node/src/checkpoint_coordinator.rs` (whole module)

Background service: polls duties, elects itself leader (the shipped duty query
always set `is_leader: true` — leader election on the node side was TODO),
collects peer signatures over HTTP, and submits `provider_checkpoint`.

```rust
/// Configuration for the checkpoint coordinator.
#[derive(Clone, Debug)]
pub struct CheckpointCoordinatorConfig {
    /// How often to poll for checkpoint duties.
    pub poll_interval: Duration,
    /// Timeout for collecting signatures from peers.
    pub signature_timeout: Duration,
    /// Whether to automatically submit checkpoints when leader.
    pub auto_submit: bool,
}

impl Default for CheckpointCoordinatorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(6), // ~1 block
            signature_timeout: Duration::from_secs(30),
            auto_submit: true,
        }
    }
}

/// Information about a checkpoint duty.
#[derive(Clone, Debug)]
pub struct CheckpointDuty {
    /// Bucket needing a checkpoint.
    pub bucket_id: BucketId,
    /// Current checkpoint window number.
    pub window: u64,
    /// Current MMR root for the bucket.
    pub mmr_root: H256,
    /// Start sequence number.
    pub start_seq: u64,
    /// Number of leaves in the MMR.
    pub leaf_count: u64,
    /// Whether this provider is the leader for this window.
    pub is_leader: bool,
    /// List of peer provider endpoints.
    pub peer_endpoints: Vec<String>,
    /// Interval in blocks.
    pub interval: u32,
    /// Grace period in blocks.
    pub grace_period: u32,
}

/// Result of a checkpoint coordination attempt.
#[derive(Clone, Debug)]
pub enum CheckpointResult {
    /// Successfully submitted checkpoint.
    Success {
        bucket_id: BucketId,
        window: u64,
        mmr_root: H256,
        signers: Vec<String>,
    },
    /// Not enough signatures collected.
    InsufficientSignatures {
        bucket_id: BucketId,
        window: u64,
        collected: usize,
        required: usize,
    },
    /// Failed to submit checkpoint transaction.
    SubmissionFailed {
        bucket_id: BucketId,
        window: u64,
        error: String,
    },
    /// Not the leader and within grace period.
    NotLeader { bucket_id: BucketId, window: u64 },
    /// Checkpoint already submitted for this window.
    AlreadySubmitted { bucket_id: BucketId, window: u64 },
}

/// Trait abstracting chain interactions for the checkpoint coordinator.
#[async_trait::async_trait]
pub trait CheckpointChainClient: Send + Sync {
    /// Get the current block number.
    async fn get_current_block(&self) -> Result<u64, Error>;

    /// Fetch checkpoint config (interval, grace_period) for a bucket.
    /// Returns `None` if no config exists on chain.
    async fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error>;

    /// Submit a checkpoint transaction with collected signatures.
    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error>;
}

#[async_trait::async_trait]
impl<T: CheckpointChainClient> CheckpointChainClient for Arc<T> {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.as_ref().get_current_block().await
    }

    async fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error> {
        self.as_ref().fetch_checkpoint_config(bucket_id).await
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        self.as_ref().submit_checkpoint(duty, signatures).await
    }
}

/// Commands for controlling the coordinator.
#[derive(Debug)]
pub enum CoordinatorCommand {
    /// Stop the coordinator.
    Stop,
    /// Pause automatic checkpoints.
    Pause,
    /// Resume automatic checkpoints.
    Resume,
    /// Force checkpoint for a specific bucket.
    ForceCheckpoint(BucketId),
}

/// Handle for controlling the checkpoint coordinator.
pub struct CheckpointCoordinatorHandle {
    command_tx: mpsc::Sender<CoordinatorCommand>,
    running: Arc<AtomicBool>,
}

impl CheckpointCoordinatorHandle {
    /// Check if the coordinator is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the coordinator.
    pub async fn stop(&self) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::Stop)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Pause automatic checkpoints.
    pub async fn pause(&self) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::Pause)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Resume automatic checkpoints.
    pub async fn resume(&self) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::Resume)
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Force a checkpoint submission for a specific bucket.
    pub async fn force_checkpoint(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.command_tx
            .send(CoordinatorCommand::ForceCheckpoint(bucket_id))
            .await
            .map_err(|_| Error::Internal("Coordinator channel closed".to_string()))
    }

    /// Get a clone of the command sender (for sharing with the HTTP API).
    pub fn command_sender(&self) -> mpsc::Sender<CoordinatorCommand> {
        self.command_tx.clone()
    }
}

/// Checkpoint coordinator service.
pub struct CheckpointCoordinator {
    config: CheckpointCoordinatorConfig,
    state: Arc<ProviderState>,
    chain_client: Box<dyn CheckpointChainClient>,
    http_client: reqwest::Client,
}

impl CheckpointCoordinator {
    /// Create a new checkpoint coordinator.
    pub fn new(
        config: CheckpointCoordinatorConfig,
        state: Arc<ProviderState>,
        chain_client: Box<dyn CheckpointChainClient>,
    ) -> Self {
        Self {
            config,
            state,
            chain_client,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Start the checkpoint coordinator background service.
    pub async fn start(
        self,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) -> Result<CheckpointCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<CoordinatorCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let running_exit = running.clone();
        tokio::spawn(async move {
            self.run_loop(command_rx, running_clone, callback).await;
            tracing::error!("Checkpoint coordinator run_loop exited unexpectedly!");
            running_exit.store(false, Ordering::SeqCst);
        });

        Ok(CheckpointCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Main coordinator loop.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<CoordinatorCommand>,
        running: Arc<AtomicBool>,
        callback: Option<Arc<dyn Fn(CheckpointResult) + Send + Sync>>,
    ) {
        let mut paused = false;
        let mut interval = tokio::time::interval(self.config.poll_interval);

        tracing::info!("Checkpoint coordinator started");

        loop {
            tokio::select! {
                // Prefer control commands over the poll tick: the interval's
                // first tick fires immediately, so an unbiased select could
                // service a poll before a Pause/Stop queued right after start().
                biased;

                cmd = command_rx.recv() => {
                    match cmd {
                        Some(CoordinatorCommand::Stop) | None => {
                            tracing::info!("Checkpoint coordinator stopping");
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        Some(CoordinatorCommand::Pause) => {
                            tracing::info!("Checkpoint coordinator paused");
                            paused = true;
                        }
                        Some(CoordinatorCommand::Resume) => {
                            tracing::info!("Checkpoint coordinator resumed");
                            paused = false;
                        }
                        Some(CoordinatorCommand::ForceCheckpoint(bucket_id)) => {
                            tracing::info!("Force checkpoint requested for bucket {}", bucket_id);
                            match self.get_checkpoint_duty(bucket_id).await {
                                Ok(Some(duty)) => {
                                    let result = self.coordinate_checkpoint(&duty).await;
                                    tracing::info!("Force checkpoint result: {:?}", result);
                                    if let Some(ref cb) = callback {
                                        cb(result);
                                    }
                                }
                                Ok(None) => {
                                    tracing::warn!("No checkpoint duty found for bucket {}", bucket_id);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to get checkpoint duty for bucket {}: {}", bucket_id, e);
                                }
                            }
                        }
                    }
                }
                _ = interval.tick() => {
                    if paused || !self.config.auto_submit {
                        continue;
                    }

                    // Get active checkpoint duties
                    match self.get_active_checkpoint_duties().await {
                        Ok(duties) => {
                            for duty in duties {
                                if duty.is_leader {
                                    tracing::info!(
                                        "Leader for checkpoint: bucket {} window {}",
                                        duty.bucket_id,
                                        duty.window
                                    );

                                    let result = self.coordinate_checkpoint(&duty).await;
                                    if let Some(ref cb) = callback {
                                        cb(result);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get checkpoint duties: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Get checkpoint duties for buckets where this provider is involved.
    async fn get_active_checkpoint_duties(&self) -> Result<Vec<CheckpointDuty>, Error> {
        // TODO: Query chain for buckets where this provider is a primary provider
        // and where provider-initiated checkpoints are enabled.
        // For now, return empty - duties would be derived from on-chain state.
        Ok(vec![])
    }

    /// Get checkpoint duty for a specific bucket.
    pub async fn get_checkpoint_duty(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<CheckpointDuty>, Error> {
        // Get bucket data from local storage
        let bucket = match self.state.storage.get_bucket(bucket_id) {
            Some(b) => b,
            None => {
                tracing::warn!("Bucket {} not found in local storage", bucket_id);
                return Ok(None);
            }
        };

        if bucket.leaf_count == 0 {
            tracing::warn!("Bucket {} has no data (leaf_count=0)", bucket_id);
            return Ok(None);
        }

        let anchor_block = self.chain_client.get_current_block().await?;

        let (interval, grace_period) = self
            .chain_client
            .fetch_checkpoint_config(bucket_id)
            .await?
            .unwrap_or((100u32, 20u32));

        let window = if interval > 0 {
            anchor_block / interval as u64
        } else {
            0
        };

        tracing::info!(
            "Checkpoint duty: bucket={} block={} interval={} window={} mmr_root=0x{} leaves={}",
            bucket_id,
            anchor_block,
            interval,
            window,
            hex::encode(&bucket.mmr_root.as_bytes()[..4]),
            bucket.leaf_count
        );

        let duty = CheckpointDuty {
            bucket_id,
            window,
            mmr_root: bucket.mmr_root,
            start_seq: bucket.start_seq,
            leaf_count: bucket.leaf_count,
            is_leader: true, // Force checkpoint bypasses leader check
            peer_endpoints: vec![],
            interval,
            grace_period,
        };

        Ok(Some(duty))
    }

    /// Coordinate a checkpoint: collect signatures and submit.
    pub async fn coordinate_checkpoint(&self, duty: &CheckpointDuty) -> CheckpointResult {
        tracing::info!(
            "Coordinating checkpoint for bucket {} window {}",
            duty.bucket_id,
            duty.window
        );

        // Step 1: Create the checkpoint proposal
        let proposal = CheckpointProposal::new(
            duty.bucket_id,
            duty.mmr_root,
            duty.start_seq,
            duty.leaf_count,
            duty.window,
        );

        // Step 2: Sign the proposal ourselves
        let our_signature = match self.sign_proposal(&proposal) {
            Some(sig) => sig,
            None => {
                return CheckpointResult::SubmissionFailed {
                    bucket_id: duty.bucket_id,
                    window: duty.window,
                    error: "No signer configured".to_string(),
                };
            }
        };

        // Step 3: Collect signatures from peers
        let mut signatures = vec![(self.state.provider_id.clone(), our_signature)];

        for endpoint in &duty.peer_endpoints {
            match self.request_signature(endpoint, &proposal).await {
                Ok(response) => {
                    if response.agreed {
                        signatures.push((response.signer, response.signature));
                    } else {
                        tracing::warn!(
                            "Peer {} disagreed with proposal (their root: {:?})",
                            endpoint,
                            response.local_mmr_root
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to get signature from {}: {}", endpoint, e);
                }
            }
        }

        // Step 4: Check if we have enough signatures
        let min_required = 1; // Would get from chain (bucket.min_providers)
        if signatures.len() < min_required {
            return CheckpointResult::InsufficientSignatures {
                bucket_id: duty.bucket_id,
                window: duty.window,
                collected: signatures.len(),
                required: min_required,
            };
        }

        // Step 5: Submit the checkpoint
        let signers: Vec<String> = signatures.iter().map(|(s, _)| s.clone()).collect();
        match self.chain_client.submit_checkpoint(duty, signatures).await {
            Ok(_) => CheckpointResult::Success {
                bucket_id: duty.bucket_id,
                window: duty.window,
                mmr_root: duty.mmr_root,
                signers,
            },
            Err(e) => CheckpointResult::SubmissionFailed {
                bucket_id: duty.bucket_id,
                window: duty.window,
                error: e.to_string(),
            },
        }
    }

    /// Sign a checkpoint proposal.
    fn sign_proposal(&self, proposal: &CheckpointProposal) -> Option<String> {
        let keypair = self.state.keypair.as_ref()?;
        let encoded = proposal.encode();
        let signature = keypair.sign(&encoded);
        Some(format!("0x{}", hex::encode(signature.0)))
    }

    /// Request a signature from a peer provider.
    async fn request_signature(
        &self,
        endpoint: &str,
        proposal: &CheckpointProposal,
    ) -> Result<SignProposalResponse, Error> {
        let url = format!("{endpoint}/checkpoint/sign");

        let request = SignProposalRequest {
            bucket_id: proposal.bucket_id,
            mmr_root: format!("0x{}", hex::encode(proposal.commitment.mmr_root.as_bytes())),
            start_seq: proposal.commitment.start_seq,
            leaf_count: proposal.commitment.leaf_count,
            window: proposal.window,
        };

        let response = self
            .http_client
            .post(&url)
            .json(&request)
            .timeout(self.config.signature_timeout)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Internal(format!(
                "Peer returned error: {}",
                response.status()
            )));
        }

        response
            .json::<SignProposalResponse>()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse response: {e}")))
    }
}

/// Request to sign a checkpoint proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignProposalRequest {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub window: u64,
}

/// Response from signing a checkpoint proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignProposalResponse {
    /// Signer's account ID.
    pub signer: String,
    /// Signature over the proposal (if agreed).
    pub signature: String,
    /// Whether the signer agreed with the proposal.
    pub agreed: bool,
    /// Signer's local MMR root (for debugging disagreements).
    pub local_mmr_root: Option<String>,
}

/// Query for checkpoint duty status.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CheckpointDutyQuery {
    pub bucket_id: BucketId,
}

/// Response with checkpoint duty information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointDutyResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub ready: bool,
}
```

### HTTP endpoints (`provider-node/src/api.rs`)

Routes:

```rust
// Checkpoint coordination
.route("/checkpoint/sign", post(sign_checkpoint_proposal))
.route("/checkpoint/duty", get(get_checkpoint_duty))
.route("/checkpoint/trigger", post(trigger_checkpoint))
```

Handlers (plus `TriggerCheckpointResponse { bucket_id, triggered, message }`
in `types.rs`):

```rust
/// Sign a checkpoint proposal from another provider.
///
/// Verifies that the proposal matches our local state and returns a signature
/// if agreed, or disagreement info if our state differs.
async fn sign_checkpoint_proposal(
    State(state): State<Arc<ProviderState>>,
    Json(request): Json<SignProposalRequest>,
) -> Result<Json<SignProposalResponse>, Error> {
    // Get our local bucket state
    let bucket = state
        .storage
        .get_bucket(request.bucket_id)
        .ok_or(Error::BucketNotFound(request.bucket_id))?;

    let local_mmr_root = format!("0x{}", hex_encode(bucket.mmr_root.as_bytes()));

    // Check if we agree with the proposal
    let proposed_root_bytes = hex_decode(&request.mmr_root).map_err(|_| Error::InvalidHash {
        expected: request.mmr_root.clone(),
        actual: "invalid hex".to_string(),
    })?;
    let proposed_root = H256::from_slice(&proposed_root_bytes);

    // We agree if MMR roots match and sequence numbers are compatible
    let agreed = bucket.mmr_root == proposed_root
        && bucket.start_seq == request.start_seq
        && bucket.leaf_count == request.leaf_count;

    if !agreed {
        return Ok(Json(SignProposalResponse {
            signer: state.provider_id.clone(),
            signature: String::new(),
            agreed: false,
            local_mmr_root: Some(local_mmr_root),
        }));
    }

    // Sign the proposal
    let proposal = CheckpointProposal::new(
        request.bucket_id,
        proposed_root,
        request.start_seq,
        request.leaf_count,
        request.window,
    );
    let encoded = proposal.encode();

    let signature = state.sign(&encoded)?;

    Ok(Json(SignProposalResponse {
        signer: state.provider_id.clone(),
        signature,
        agreed: true,
        local_mmr_root: Some(local_mmr_root),
    }))
}

/// Trigger checkpoint submission for a bucket.
///
/// Sends a ForceCheckpoint command to the checkpoint coordinator,
/// which handles leader election, signature collection, and on-chain submission.
async fn trigger_checkpoint(
    State(state): State<Arc<ProviderState>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CheckpointDutyQuery>,
) -> Result<Json<TriggerCheckpointResponse>, Error> {
    check_role(
        &state,
        &headers,
        "POST",
        query.bucket_id,
        RequiredRole::Writer,
    )
    .await?;

    tracing::info!(
        "Checkpoint trigger requested for bucket {}",
        query.bucket_id
    );

    // Verify bucket has data locally
    let bucket = state.storage.get_bucket(query.bucket_id);
    if bucket.is_none() {
        return Err(Error::Internal(format!(
            "Bucket {} not found in local storage. Upload data first.",
            query.bucket_id
        )));
    }
    let bucket = bucket.unwrap();
    if bucket.leaf_count == 0 {
        return Err(Error::Internal(format!(
            "Bucket {} has no committed data (leaf_count=0). Upload and commit data first.",
            query.bucket_id
        )));
    }

    let sender = state
        .checkpoint_cmd_tx
        .lock()
        .map_err(|_| Error::Internal("Lock poisoned".to_string()))?
        .clone();

    let sender = sender.ok_or_else(|| {
        Error::Internal(
            "Checkpoint coordinator not running. Start provider with --enable-checkpoint-coordinator"
                .to_string(),
        )
    })?;

    sender
        .send(crate::checkpoint_coordinator::CoordinatorCommand::ForceCheckpoint(query.bucket_id))
        .await
        .map_err(|_| Error::Internal(
            "Coordinator channel closed — the coordinator task may have crashed. Check provider logs and restart.".to_string(),
        ))?;

    tracing::info!(
        "ForceCheckpoint command sent for bucket {} (leaves={}, mmr_root=0x{})",
        query.bucket_id,
        bucket.leaf_count,
        hex_encode(&bucket.mmr_root.as_bytes()[..4])
    );

    Ok(Json(TriggerCheckpointResponse {
        bucket_id: query.bucket_id,
        triggered: true,
        message: format!(
            "Checkpoint triggered for bucket {} with {} leaves. The coordinator will handle submission.",
            query.bucket_id, bucket.leaf_count
        ),
    }))
}

/// Get checkpoint duty information for a bucket.
///
/// Returns the current state that would be used for a checkpoint.
async fn get_checkpoint_duty(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<CheckpointDutyQuery>,
) -> Result<Json<CheckpointDutyResponse>, Error> {
    let bucket = state
        .storage
        .get_bucket(query.bucket_id)
        .ok_or(Error::BucketNotFound(query.bucket_id))?;

    // We're ready if we have data committed
    let ready = bucket.leaf_count > 0;

    Ok(Json(CheckpointDutyResponse {
        bucket_id: query.bucket_id,
        mmr_root: format!("0x{}", hex_encode(bucket.mmr_root.as_bytes())),
        start_seq: bucket.start_seq,
        leaf_count: bucket.leaf_count,
        ready,
    }))
}
```

### Chain client (`provider-node/src/subxt_client.rs`)

```rust
#[async_trait::async_trait]
impl CheckpointChainClient for SubxtChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.current_anchor_block().await
    }

    async fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error> {
        use subxt::dynamic::At;

        let config_query =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "CheckpointConfigs");
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match at
            .storage()
            .try_fetch(config_query, (Value::u128(bucket_id as u128),))
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch config: {e}")))?
        {
            Some(val) => {
                let decoded = val
                    .decode()
                    .map_err(|e| Error::Internal(format!("Failed to decode config: {e}")))?;
                let interval = decoded
                    .at("interval")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(100) as u32;
                let grace_period = decoded
                    .at("grace_period")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(20) as u32;
                Ok(Some((interval, grace_period)))
            }
            None => Ok(None),
        }
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let bucket_id = duty.bucket_id;
        let mmr_root = duty.mmr_root;
        let start_seq = duty.start_seq;
        let leaf_count = duty.leaf_count;
        let window = duty.window;

        // Build signature tuples for the extrinsic
        let mut sig_values = Vec::with_capacity(signatures.len());
        for (account, sig) in &signatures {
            let account_id: sp_core::crypto::AccountId32 =
                sp_core::crypto::Ss58Codec::from_ss58check(account).map_err(|e| {
                    Error::Internal(format!("Invalid SS58 account '{account}': {e:?}"))
                })?;
            let account_bytes: [u8; 32] = account_id.into();

            let sig_bytes = hex::decode(sig.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid signature hex: {e}")))?;

            sig_values.push(value!((
                Value::from_bytes(account_bytes),
                Sr25519(Value::from_bytes(sig_bytes))
            )));
        }

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "provider_checkpoint",
            vec![
                Value::u128(bucket_id as u128),
                Value::named_composite(vec![
                    ("mmr_root", Value::from_bytes(mmr_root.as_bytes())),
                    ("start_seq", Value::u128(start_seq as u128)),
                    ("leaf_count", Value::u128(leaf_count as u128)),
                ]),
                Value::u128(window as u128),
                Value::unnamed_composite(sig_values),
            ],
        );

        let tx_progress = self
            .api
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get current block: {e}")))?
            .transactions()
            .sign_and_submit_then_watch_default(&tx, &self.signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        let _events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

        Ok(H256::zero())
    }
}
```

### CLI + startup wiring

`cli.rs` had a `CheckpointParams { enable_checkpoint_coordinator }` flattened
group (`--enable-checkpoint-coordinator` / `ENABLE_CHECKPOINT_COORDINATOR`),
and `ProviderState` carried
`checkpoint_cmd_tx: Mutex<Option<mpsc::Sender<CoordinatorCommand>>>` +
`set_checkpoint_handle()` so the `/checkpoint/trigger` handler could reach the
coordinator. `command.rs`:

```rust
async fn start_checkpoint_coordinator(
    cli: &Cli,
    chain_client: Option<&SubxtChainClient>,
    state: Arc<ProviderState>,
) -> Option<CheckpointCoordinatorHandle> {
    if !cli.checkpoint.enable_checkpoint_coordinator {
        return None;
    }

    let chain_client = match chain_client {
        Some(c) => c.clone(),
        None => {
            tracing::error!(
                "Checkpoint coordinator needs a chain client (--keyfile + reachable chain). Disabled."
            );
            return None;
        }
    };

    let config = CheckpointCoordinatorConfig::default();

    let coordinator = CheckpointCoordinator::new(config, state, Box::new(chain_client));

    match coordinator.start(None).await {
        Ok(handle) => {
            tracing::info!("Checkpoint coordinator started");
            Some(handle)
        }
        Err(e) => {
            tracing::error!("Failed to start checkpoint coordinator: {}", e);
            None
        }
    }
}
```

### Provider-node tests removed

- `tests/coordinators/checkpoint.rs` (whole file): `MockCheckpointChainClient`
  (block/config/submit-result knobs); tests `test_config_default`,
  `test_sign_proposal_request_serialization`, `test_no_bucket_data`,
  `test_duty_found_submit_ok`, `duty_window_derives_from_relay_scale_block`
  (window = relay_block / interval on the relay scale),
  `duty_window_moves_only_on_interval_boundary`, `test_submit_fails`,
  `test_pause_resume`, `test_force_checkpoint`.
- `tests/api_integration.rs`: `checkpoint_sign_endpoint_returns_503_when_no_signing_key`
  (the endpoint must refuse rather than emit a zero signature),
  `test_checkpoint_sign_happy_path`, `test_checkpoint_sign_disagreement`
  (`agreed: false`, empty signature on root mismatch),
  `test_checkpoint_duty_endpoint`, `test_checkpoint_trigger_no_coordinator`
  (500 when coordinator absent), `test_checkpoint_trigger_unknown_bucket`.
- `tests/auth_integration.rs`: `checkpoint_trigger_missing_auth_returns_401`,
  `checkpoint_trigger_reader_blocked` (trigger is Writer-level).

## Extrinsics reference entries (removed from `docs/reference/EXTRINSICS_REFERENCE.md`)

---

### `providerCheckpoint`

**Provider-initiated checkpoint.** Primary providers coordinate autonomously and submit on a fixed window cadence. Submitter receives `CheckpointReward` from the bucket's checkpoint pool (if funded).

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `mmrRoot`: `H256`
- `startSeq`: `u64`
- `leafCount`: `u64`
- `window`: `u64` - current window number (replay protection)
- `signatures`: `BoundedVec<(AccountId, MultiSignature), T::MaxPrimaryProviders>`

**Example:**
```
bucketId: 0
mmrRoot: 0x1234567890abcdef...
startSeq: 0
leafCount: 10
window: 42
signatures: [
  (5GrwvaEF..., 0xsig1...),
  (5FHneW46..., 0xsig2...)
]
```

**Leader election:** `blake2_256(bucket_id || window) % primary_count`. During the grace period only the elected leader may submit; afterwards any primary may submit (fallback).

**Events:** `ProviderCheckpointSubmitted { window, leader, signers, reward }`
**Errors:** `BucketNotFound`, `ProviderCheckpointsDisabled`, `InvalidCheckpointWindow`, `CheckpointAlreadySubmitted`, `NotCheckpointLeader`, `ProviderNotInSnapshot`, `InvalidSignature`, `SnapshotViolatesFrozen`, `MinProvidersNotMet`, `InsufficientSignatures`

---

### `configureCheckpointWindow`

Admin configuration for provider-initiated checkpoints on a bucket.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `interval`: `BlockNumber` - blocks per window
- `gracePeriod`: `BlockNumber` - leader-only window at the start
- `enabled`: `bool`

**Example:**
```
bucketId: 0
interval: 100
gracePeriod: 20
enabled: true
```

**Events:** `CheckpointConfigUpdated`
**Errors:** `BucketNotFound`, `NotBucketAdmin`

---

### `reportMissedCheckpoint`

**Permissionless.** If a window's grace period expires without a checkpoint, anyone can report the leader: the leader is slashed by `CheckpointMissPenalty` and the reporter receives 10% as a bounty.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `window`: `u64`

**Example:**
```
bucketId: 0
window: 42
```

**Events:** `CheckpointMissPenalized`
**Errors:** `BucketNotFound`, `ProviderCheckpointsDisabled`, `InvalidCheckpointWindow`, `CheckpointAlreadySubmitted`, `WithinGracePeriod`, `ProviderNotInSnapshot`

---

### `claimCheckpointRewards`

Provider claims accumulated rewards for a bucket.

**Parameters:**
- `bucketId`: `BucketId` (u64)

**Example:**
```
bucketId: 0
```

**Events:** `CheckpointRewardClaimed`
**Errors:** `NoRewardsToClaim`

---

### `fundCheckpointPool`

**Permissionless.** Anyone can top up a bucket's reward pool to incentivize provider-initiated checkpoints.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `amount`: `Balance` - amount to add to the pool

**Example:**
```
bucketId: 0
amount: 100000000000000   // 100 tokens
```

## Example: `examples/papi/checkpoint-missed.ts` (deleted, was `just papi-checkpoint-missed`)

```ts
// SPDX-License-Identifier: Apache-2.0

/**
 * Missed-checkpoint reporting flow for pallet-storage-provider.
 *
 * Demonstrates that when a bucket has a checkpoint window configured but no
 * provider submits a `provider_checkpoint` for that window, anyone can call
 * `report_missed_checkpoint` once the window has fully passed. The pallet
 * slashes the elected leader's reserved stake and pays the reporter 10%.
 *
 * Exercised extrinsics:
 *   - configure_checkpoint_window  (tight interval so the demo runs in <2 min)
 *   - report_missed_checkpoint     (the slashing path)
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider node running at the specified URL (its checkpoint coordinator
 *     must NOT be enabled, otherwise it would auto-submit and there would be
 *     no missed window to report)
 *   - Workspace deps installed: pnpm install (descriptors come from the tracked metadata)
 *
 * Usage: node checkpoint-missed.js [chain_ws] [provider_url] [provider_seed] [client_seed]
 */

import assert from "node:assert";
import {
  configureCheckpointWindow,
  connect,
  currentRelayBlock,
  ensureProviderRegistered,
  establishStorageAgreement,
  makeSigner,
  negotiateTerms,
  READ_OPTS,
  reportMissedCheckpoint,
  sameAddress,
  waitForRelayBlock,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "@web3-storage/sdk";
import {
  ensureSoleAcceptingProvider,
  parseProviderClientArgs,
} from "./support.js";

const {
  chainWs: CHAIN_WS,
  providerUrl: PROVIDER_URL,
  providerSeed: PROVIDER_SEED,
  clientSeed: CLIENT_SEED,
} = parseProviderClientArgs();

// Tight window so the demo finishes quickly. report_missed_checkpoint requires
// current_block > (window + 1) * interval, so the longest we ever wait is
// `interval` blocks (~60s at 6s blocks).
const WINDOW_INTERVAL = 10;
const WINDOW_GRACE = 5;

async function main() {
  const provider = makeSigner(PROVIDER_SEED);
  const client = makeSigner(CLIENT_SEED);

  console.log("Chain:", CHAIN_WS, " Provider HTTP:", PROVIDER_URL);
  console.log("Provider (%s) => %s", PROVIDER_SEED, provider.address);
  console.log("Reporter (%s) => %s", CLIENT_SEED, client.address);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  let restoreOthers = null;
  try {
    console.log("\n=== Step 1: Setup provider + bucket + agreement ===");
    await ensureProviderRegistered(api, provider, PROVIDER_URL);
    restoreOthers = await ensureSoleAcceptingProvider(api, provider);

    const signed = await negotiateTerms(PROVIDER_URL, {
      owner: client.address,
      max_bytes: 1_048_576n, // 1 MiB
      duration: 200,
      price_per_byte: 1n,
      replica_params: null,
      bucket_id: null,
    });
    const { bucketId } = await establishStorageAgreement(api, client, provider, signed);
    console.log("  Bucket + agreement opened: id=%s", bucketId);

    const bucket = (await api.query.StorageProvider.Buckets.getValue(
      bucketId,
      READ_OPTS
    ))!;
    assert.ok(
      bucket.primary_providers.some((p: string) => sameAddress(p, provider.address)),
      "Provider should be primary after establish_storage_agreement"
    );

    console.log("\n=== Step 2: configure_checkpoint_window (tight) ===");
    await configureCheckpointWindow(api, client, bucketId, {
      interval: WINDOW_INTERVAL,
      gracePeriod: WINDOW_GRACE,
    });
    console.log(
      "  Window configured: interval=%d grace=%d",
      WINDOW_INTERVAL,
      WINDOW_GRACE
    );

    console.log("\n=== Step 3: Pick a window and let it elapse without a checkpoint ===");
    const head = await currentRelayBlock(api);
    const missedWindow = BigInt(Math.floor(head / WINDOW_INTERVAL));
    // window_end = (missedWindow + 1) * interval ; need current_block > window_end
    const windowEnd = (Number(missedWindow) + 1) * WINDOW_INTERVAL;
    console.log(
      "  head=%d  missed_window=%s  window_end=%d (must wait until head > %d)",
      head,
      missedWindow,
      windowEnd,
      windowEnd
    );
    await waitForRelayBlock(papi, api, windowEnd);

    console.log("\n=== Step 4: Record balances, then report_missed_checkpoint ===");
    const providerBefore = (await api.query.StorageProvider.Providers.getValue(
      provider.address,
      READ_OPTS
    ))!;
    const reporterAcctBefore = await api.query.System.Account.getValue(
      client.address,
      READ_OPTS
    );
    console.log("  Provider stake before: %s", providerBefore.stake.toString());
    console.log(
      "  Reporter free before:  %s",
      reporterAcctBefore.data.free.toString()
    );

    const event = await reportMissedCheckpoint(api, client, bucketId, Number(missedWindow));
    console.log(
      "  CheckpointMissPenalized: provider=%s window=%s penalty=%s",
      event.provider,
      event.window,
      event.penalty.toString()
    );
    assert.ok(
      sameAddress(event.provider, provider.address),
      `Leader should be the lone primary provider, got ${event.provider}`
    );
    assert.ok(event.penalty > 0n, "Penalty should be > 0");

    console.log("\n=== Step 5: Verify slashing + reporter reward ===");
    const providerAfter = (await api.query.StorageProvider.Providers.getValue(
      provider.address,
      READ_OPTS
    ))!;
    const stakeDelta = providerBefore.stake - providerAfter.stake;
    console.log("  Provider stake delta: %s", stakeDelta.toString());
    assert.strictEqual(
      stakeDelta,
      event.penalty,
      `Provider stake should drop by exactly the penalty (${event.penalty})`
    );

    // LastCheckpointWindow is updated to prevent re-reporting.
    const lastWindow =
      await api.query.StorageProvider.LastCheckpointWindow.getValue(
        bucketId,
        READ_OPTS
      );
    assert.strictEqual(
      lastWindow,
      missedWindow,
      `LastCheckpointWindow should record the just-reported window (${missedWindow}), got ${lastWindow}`
    );
    console.log("  LastCheckpointWindow[%s] = %s ✓", bucketId, lastWindow);

    console.log("\nPASSED: missed-checkpoint reporting + leader slashing");
  } catch (err) {
    console.error("\nERROR:", (err as Error).message || err);
    if ((err as Error).stack) console.error((err as Error).stack);
    process.exitCode = 1;
  } finally {
    if (restoreOthers) {
      try {
        await restoreOthers();
      } catch (err) {
        console.error("WARN: restoring providers failed:", (err as Error).message || err);
      }
    }
    papi.destroy();
  }
}

main().then(() => console.log("\n=== Done ==="));
```

## JS/TS SDK, examples, and UI surface (verbatim)

Everything removed from `packages/`, `examples/`, `user-interfaces/`,
`justfile`, and CI, organized by file.

### packages/layer0/src/pallets/storage-provider.ts

```ts
export async function configureCheckpointWindow(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  {
    interval,
    gracePeriod,
    enabled = true,
  }: { interval: number; gracePeriod: number; enabled?: boolean },
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.configure_checkpoint_window({
      bucket_id: bucketId,
      interval,
      grace_period: gracePeriod,
      enabled,
    }),
    admin.signer,
    { label: "configure_checkpoint_window", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointConfigUpdated,
    "CheckpointConfigUpdated",
  );
}

export async function fundCheckpointPool(
  api: ParachainApi,
  funder: ChainSigner,
  bucketId: bigint,
  amount: bigint,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.fund_checkpoint_pool({
      bucket_id: bucketId,
      amount,
    }),
    funder.signer,
    { label: "fund_checkpoint_pool", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointPoolFunded,
    "CheckpointPoolFunded",
  );
}

export async function submitProviderCheckpoint(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
  duty: { mmr_root: string; start_seq: number | string; leaf_count: number | string },
  signature: string,
  window: number,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.provider_checkpoint({
      bucket_id: bucketId,
      commitment: {
        mmr_root: asHex(duty.mmr_root),
        start_seq: BigInt(duty.start_seq),
        leaf_count: BigInt(duty.leaf_count),
      },
      window: BigInt(window),
      signatures: [[provider.address, Enum("Sr25519", asHex(signature))]],
    }),
    provider.signer,
    { label: "provider_checkpoint", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ProviderCheckpointSubmitted,
    "ProviderCheckpointSubmitted",
  );
}

export async function claimCheckpointRewards(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.claim_checkpoint_rewards({ bucket_id: bucketId }),
    provider.signer,
    { label: "claim_checkpoint_rewards", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointRewardClaimed,
    "CheckpointRewardClaimed",
  );
}

export async function reportMissedCheckpoint(
  api: ParachainApi,
  reporter: ChainSigner,
  bucketId: bigint,
  window: number,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.report_missed_checkpoint({
      bucket_id: bucketId,
      window: BigInt(window),
    }),
    reporter.signer,
    { label: "report_missed_checkpoint", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointMissPenalized,
    "CheckpointMissPenalized",
  );
}
```

### packages/layer0/src/provider-http.ts

```ts
export async function fetchCheckpointDuty(
  providerUrl: string,
  bucketId: bigint | number,
): Promise<any> {
  return providerFetch(providerUrl, "/checkpoint/duty", {
    params: { bucket_id: bucketId },
  });
}

export async function signCheckpointProposal(
  providerUrl: string,
  bucketId: bigint | number,
  duty: { mmr_root: string; start_seq: number | string; leaf_count: number | string },
  window: number | bigint,
): Promise<any> {
  return providerFetch(providerUrl, "/checkpoint/sign", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      mmr_root: duty.mmr_root,
      start_seq: duty.start_seq,
      leaf_count: duty.leaf_count,
      window: Number(window),
    },
  });
}
```

### packages/layer0/src/waits.ts (doc comment, original wording)

```ts
/**
 * Wait until the chain's best head is strictly greater than `target`.
 *
 * Callers that need to land an extrinsic at a specific block window (e.g.
 * `provider_checkpoint`, `report_missed_checkpoint`) use this to time their
 * submission so the runtime sees the block range they computed against.
 */
```

### packages/layer1/src/fs/client.ts

Methods on `FileSystemClient` (plus the `CheckpointDuty` entry in the type-import list from `./types.js`):

```ts
  // ── Checkpoint (provider HTTP) ──────────────────────────────────────────

  async getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/duty?bucket_id=${bucketId}`,
      { headers: await this.authHeaders("GET", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      if (response.status === 404) return null;
      throw new Error(`Checkpoint duty failed: ${response.status}`);
    }
    return response.json();
  }

  async triggerCheckpoint(bucketId: bigint): Promise<void> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/trigger?bucket_id=${bucketId}`,
      { method: "POST", headers: await this.authHeaders("POST", bucketId) },
      this.fetchOpts,
    );
    if (!response.ok) {
      throw new Error(`Checkpoint trigger failed: ${response.status} ${await response.text().catch(() => "")}`);
    }
  }
```

### packages/layer1/src/fs/types.ts

```ts
export interface CheckpointDuty {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  ready: boolean;
}
```

### examples/papi/checkpoint-missed.ts

deleted wholesale — full source archived separately by the orchestrator

### examples/papi/e2e/05-checkpoint-and-challenges.ts

Removed imports (from the `@web3-storage/sdk` import list): `claimCheckpointRewards`, `configureCheckpointWindow`, `fetchCheckpointDuty`, `fundCheckpointPool`, `READ_OPTS`, `signCheckpointProposal`, `submitProviderCheckpoint`, `waitForRelayBlock`.

Removed consts:

```ts
const WINDOW_INTERVAL = 20;
const WINDOW_GRACE = 10;
const POOL_AMOUNT = 5_000_000_000_000n;
```

Removed tests (the surviving "Challenge a provider not in the snapshot" test was renumbered 5.6 → 5.4; header comment "client/provider checkpoints" → "client checkpoints"):

```ts
  tests.push({
    name: "5.4 Provider-initiated checkpoint + reward",
    fn: async () => {
      await configureCheckpointWindow(api, client, bucketId, {
        interval: WINDOW_INTERVAL,
        gracePeriod: WINDOW_GRACE,
      });
      await fundCheckpointPool(api, client, bucketId, POOL_AMOUNT);

      // Compute current window with headroom.
      const HEADROOM = 8;
      let currentBlock = await currentRelayBlock(api);
      let windowNum = Math.floor(currentBlock / WINDOW_INTERVAL);
      let nextWindowStart = (windowNum + 1) * WINDOW_INTERVAL;
      if (nextWindowStart - currentBlock < HEADROOM) {
        await waitForRelayBlock(papi, api, nextWindowStart - 1);
        currentBlock = await currentRelayBlock(api);
        windowNum = Math.floor(currentBlock / WINDOW_INTERVAL);
      }
      const window = BigInt(windowNum);

      const duty = await fetchCheckpointDuty(PROVIDER_URL, bucketId);
      assert.ok(duty.ready, "Provider should be ready to checkpoint");
      const signed = await signCheckpointProposal(PROVIDER_URL, bucketId, duty, window);
      assert.ok(signed.agreed, "Provider should agree to sign");

      const event = await submitProviderCheckpoint(
        api,
        provider,
        bucketId,
        duty,
        signed.signature,
        Number(window)
      );
      assert.ok(event.reward > 0n, "Reward should be positive");
    },
  });

  tests.push({
    name: "5.5 Claim checkpoint rewards",
    fn: async () => {
      const pending = await api.query.StorageProvider.CheckpointRewards.getValue(
        provider.address,
        bucketId,
        READ_OPTS
      );
      assert.ok(pending > 0n, "Should have pending rewards");
      const event = await claimCheckpointRewards(api, provider, bucketId);
      assert.ok(event.amount > 0n, "Claimed amount should be positive");
      const after = await api.query.StorageProvider.CheckpointRewards.getValue(
        provider.address,
        bucketId,
        READ_OPTS
      );
      assert.strictEqual(after, 0n, "Rewards should be cleared after claim");
    },
  });

  tests.push({
    name: "5.7 No rewards to claim",
    fn: async () => {
      // We already claimed rewards in 5.5, so claiming again should fail.
      const tx = api.tx.StorageProvider.claim_checkpoint_rewards({ bucket_id: bucketId });
      await submitTxExpectFailure(tx, provider.signer, "NoRewardsToClaim", "5.7");
    },
  });
```

### examples/papi/sc-token-gated.ts (comment, original wording)

```ts
    // Publisher is the demo's "client" account (default //Bob), NOT the
    // storage provider. Using the provider account here would race the
    // checkpoint coordinator that signs background extrinsics from the
    // same key, surfacing as `Invalid::Stale` on the mempool side.
```

### justfile

`--enable-checkpoint-coordinator \` line removed from the `start-provider` recipe's `./target/release/storage-provider-node` invocation (was between `--chain-rpc "{{ CHAIN_WS }}" \` and `$EXTRA_ARGS`).

Removed recipe:

```make
# Missed checkpoint slashing flow: configure_checkpoint_window (tight) ->
# wait past window -> report_missed_checkpoint (slashes leader, pays reporter).
papi-checkpoint-missed PROVIDER_URL=PROVIDER_URL PROVIDER_SEED="//Alice" CLIENT_SEED="//Bob": papi-setup
    node --import tsx examples/papi/checkpoint-missed.ts "{{ CHAIN_WS }}" "{{ PROVIDER_URL }}" "{{ PROVIDER_SEED }}" "{{ CLIENT_SEED }}"
```

### .github/workflows/integration-tests.yml

`--enable-checkpoint-coordinator` removed from 4 provider-node launch commands; each occurrence looked like:

```yaml
          ./target/release/storage-provider-node \
            ... \
            --chain-rpc ws://127.0.0.1:2222 \
            --enable-checkpoint-coordinator > /tmp/<name>.log 2>&1 &
```

### user-interfaces/provider/src/lib/chain-client.ts

```ts
export interface OnChainCheckpointConfig {
  interval: number
  gracePeriod: number
  enabled: boolean
}
```

Fields removed from `OnChainBucketDetails`:

```ts
  checkpointConfig: OnChainCheckpointConfig | null
  lastCheckpointWindow: bigint | null
  checkpointPoolBalance: bigint
  checkpointReward: bigint
```

Query blocks removed from `getBucketDetails` (the `providerAddress: string` parameter, used only by the rewards query, was removed along with them; the pushed object lost the four fields below):

```ts
    let checkpointConfig: OnChainCheckpointConfig | null = null
    try {
      const config = await a.query.StorageProvider.CheckpointConfigs.getValue(BigInt(bucketId))
      if (config) {
        checkpointConfig = {
          interval: config.interval,
          gracePeriod: config.grace_period,
          enabled: config.enabled,
        }
      }
    } catch { /* not configured */ }

    let lastCheckpointWindow: bigint | null = null
    try {
      const val = await a.query.StorageProvider.LastCheckpointWindow.getValue(BigInt(bucketId))
      lastCheckpointWindow = val ?? null
    } catch { /* not set */ }

    let checkpointPoolBalance = 0n
    try {
      const pool = await a.query.StorageProvider.CheckpointPool.getValue(BigInt(bucketId))
      checkpointPoolBalance = pool ?? 0n
    } catch { /* no pool */ }

    let checkpointReward = 0n
    try {
      const reward = await a.query.StorageProvider.CheckpointRewards.getValue(
        providerAddress,
        BigInt(bucketId),
      )
      checkpointReward = reward ?? 0n
    } catch { /* no reward */ }
```

Fields dropped from the object pushed in `getBucketDetails`:

```ts
      checkpointConfig,
      lastCheckpointWindow,
      checkpointPoolBalance,
      checkpointReward,
```

### user-interfaces/provider/src/state/provider.state.ts

Fields removed from `BucketDetail`:

```ts
  checkpointConfig: { interval: number; gracePeriod: number; enabled: boolean } | null
  lastCheckpointWindow: bigint | null
  checkpointPoolBalance: bigint
  checkpointReward: bigint
  isCheckpointOverdue: boolean
```

Overdue computation removed from `loadProviderData` (the surrounding `.map` now calls `convertBucketDetail(bd, agr || null)`; the `getBucketDetails(bucketIds, address)` call lost its second argument and the `getAnchorBlock` import became unused):

```ts
        // Checkpoint windows are anchor-block / interval, so overdue detection
        // must divide the anchor clock, not the parachain height.
        const anchorBlock = getAnchorBlock() || 0
```
```ts
            let isOverdue = false
            if (bd.checkpointConfig?.enabled && bd.lastCheckpointWindow != null && anchorBlock > 0) {
              const expectedWindow = BigInt(Math.floor(anchorBlock / bd.checkpointConfig.interval))
              isOverdue = expectedWindow > bd.lastCheckpointWindow + 1n
            }
            return convertBucketDetail(bd, agr || null, isOverdue)
```

`convertBucketDetail` — removed the `isCheckpointOverdue: boolean` third parameter and these returned fields:

```ts
    checkpointConfig: chain.checkpointConfig,
    lastCheckpointWindow: chain.lastCheckpointWindow,
    checkpointPoolBalance: chain.checkpointPoolBalance,
    checkpointReward: chain.checkpointReward,
    isCheckpointOverdue,
```

### user-interfaces/provider/src/pages/Buckets.tsx

Removed computation (in `BucketsContent`):

```tsx
  const overdueCount = buckets.filter((b) => b.isCheckpointOverdue).length
```

Removed stat tile (the stats grid went from `md:grid-cols-3` to `md:grid-cols-2`):

```tsx
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Overdue Checkpoints</CardTitle>
          </CardHeader>
          <CardContent>
            <div className={`text-2xl font-bold ${overdueCount > 0 ? 'text-yellow-400' : 'text-green-400'}`}>
              {overdueCount}
            </div>
          </CardContent>
        </Card>
```

Removed overdue badge branch in the Checkpoint table cell (replaced by an unconditional `CheckCircle`; the `AlertTriangle` import became unused):

```tsx
                              {bucket.isCheckpointOverdue ? (
                                <AlertTriangle className="h-3 w-3 text-yellow-400" />
                              ) : (
                                <CheckCircle className="h-3 w-3 text-green-400" />
                              )}
```

Removed "Checkpoint Config" section from `BucketExpandedDetail` (the `formatDuration` import became unused):

```tsx
        {bucket.checkpointConfig && (
          <div>
            <h4 className="text-sm font-medium text-gray-400 mb-2">Checkpoint Config</h4>
            <div className="grid grid-cols-3 gap-2 text-sm">
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">Interval</span>
                <p className="font-medium">{formatDuration(bucket.checkpointConfig.interval)}</p>
              </div>
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">Grace Period</span>
                <p className="font-medium">{formatDuration(bucket.checkpointConfig.gracePeriod)}</p>
              </div>
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">Enabled</span>
                <p className="font-medium">{bucket.checkpointConfig.enabled ? 'Yes' : 'No'}</p>
              </div>
            </div>
          </div>
        )}
```

Removed "Checkpoint Rewards" section from `BucketExpandedDetail` (the column comment was "Right: Snapshot + Rewards"):

```tsx
        <div>
          <h4 className="text-sm font-medium text-gray-400 mb-2">Checkpoint Rewards</h4>
          <div className="grid grid-cols-2 gap-2 text-sm">
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Pool Balance</span>
              <p className="font-medium">{formatTokens(bucket.checkpointPoolBalance)}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Your Pending Reward</span>
              <p className="font-medium text-green-400">{formatTokens(bucket.checkpointReward)}</p>
            </div>
          </div>
        </div>
```

### user-interfaces/s3-ui/src/lib/s3-client.ts

```ts
export interface CheckpointDuty {
  bucketId: number;
  mmrRoot: string;
  startSeq: number;
  leafCount: number;
  ready: boolean;
}
```

Methods on `S3Client` (the section header comment was "── Checkpoint (chain-state read + provider HTTP duty/trigger) ──"):

```ts
  async getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    const providerUrl = await this.getProviderUrl(bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/duty?bucket_id=${Number(bucketId)}`,
    );
    if (!response.ok) {
      if (response.status === 404) return null;
      throw new Error(`Checkpoint duty failed: ${response.status}`);
    }
    return response.json();
  }

  async triggerCheckpoint(bucketId: bigint): Promise<void> {
    if (!this.signer) throw new Error("Connect a wallet to trigger a checkpoint");
    const providerUrl = await this.getProviderUrl(bucketId);
    const headers = await signProviderRequest(this.signer, "POST", bucketId);
    const response = await httpFetch(
      `${providerUrl}/checkpoint/trigger?bucket_id=${Number(bucketId)}`,
      { method: "POST", headers },
    );
    if (!response.ok) {
      throw new Error(
        `Checkpoint trigger failed: ${response.status} ${await response.text().catch(() => "")}`,
      );
    }
  }
```

### user-interfaces/s3-ui/src/state/checkpoint.state.ts

Removed duty state, the trigger action, and its status/polling lifecycle (kept: `checkpointInfo$`, `checkpointLoading$`, `useCheckpointInfo`, `useCheckpointLoading`, `refreshCheckpoint`, `clearCheckpointState`):

```ts
export type CheckpointStatus = "idle" | "triggering" | "polling" | "confirmed";

const checkpointDuty$ = new BehaviorSubject<CheckpointDuty | null>(null);
const checkpointStatus$ = new BehaviorSubject<CheckpointStatus>("idle");

export const [useCheckpointDuty] = bind(checkpointDuty$, null);
export const [useCheckpointStatus] = bind(checkpointStatus$, "idle" as CheckpointStatus);

let pollTimer: ReturnType<typeof setInterval> | null = null;

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}
```

Inside `refreshCheckpoint`, the duty half of the parallel fetch:

```ts
    const [info, duty] = await Promise.all([
      client.getCheckpointInfo(bucketId).catch(() => null),
      client.getCheckpointDuty(bucketId).catch(() => null),
    ]);
    checkpointInfo$.next(info);
    checkpointDuty$.next(duty);
```

```ts
const POLL_INTERVAL_MS = 4_000;
const POLL_MAX_MS = 120_000;
const CONFIRMED_DISPLAY_MS = 4_000;

export async function triggerCheckpoint(bucketId: bigint): Promise<void> {
  stopPolling();
  checkpointStatus$.next("triggering");

  const client = getS3Client();
  try {
    await client.triggerCheckpoint(bucketId);
  } catch (err) {
    checkpointStatus$.next("idle");
    throw err;
  }

  // Capture the block number before trigger so we can detect a new checkpoint
  const blockBefore = checkpointInfo$.getValue()?.checkpointBlock ?? -1;

  checkpointStatus$.next("polling");
  const startedAt = Date.now();

  pollTimer = setInterval(async () => {
    try {
      await refreshCheckpoint(bucketId);
      const current = checkpointInfo$.getValue();
      const newBlock = current?.checkpointBlock ?? -1;
      if (newBlock > blockBefore) {
        // New checkpoint landed on-chain
        stopPolling();
        checkpointStatus$.next("confirmed");
        setTimeout(() => {
          if (checkpointStatus$.getValue() === "confirmed") {
            checkpointStatus$.next("idle");
          }
        }, CONFIRMED_DISPLAY_MS);
      }
    } catch {
      // Ignore transient errors during polling
    }

    if (Date.now() - startedAt > POLL_MAX_MS) {
      stopPolling();
      checkpointStatus$.next("idle");
    }
  }, POLL_INTERVAL_MS);
}
```

`clearCheckpointState` also lost its `stopPolling()` and `checkpointDuty$.next(null)` / `checkpointStatus$.next("idle")` lines.

### user-interfaces/s3-ui/src/state/index.ts

Removed from the checkpoint export block: `useCheckpointDuty`, `useCheckpointStatus`, `triggerCheckpoint`, `type CheckpointStatus`.

### user-interfaces/s3-ui/src/components/CheckpointPanel.tsx

Removed duty display, trigger button/handler, and the trigger status banners (the `busy` guard and the `useCheckpointDuty`/`useCheckpointStatus`/`triggerCheckpoint`/`CheckCircle2` imports went with them):

```tsx
  const duty = useCheckpointDuty();
  const status = useCheckpointStatus();
```

```tsx
  const busy = status === "triggering" || status === "polling";
```

```tsx
  const handleTrigger = async () => {
    if (bucketId === null) return;
    try {
      await triggerCheckpoint(bucketId);
    } catch (err) {
      toast({
        title: "Checkpoint failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };
```

```tsx
        {/* Checkpoint progress status */}
        {status === "triggering" && (
          <div className="flex items-center gap-2 rounded-md bg-blue-500/15 px-3 py-2 text-sm text-blue-600 dark:text-blue-400 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Sending checkpoint trigger...
          </div>
        )}
        {status === "polling" && (
          <div className="flex items-center gap-2 rounded-md bg-orange-500/15 px-3 py-2 text-sm text-orange-600 dark:text-orange-400 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Waiting for on-chain confirmation...
          </div>
        )}
        {status === "confirmed" && (
          <div className="flex items-center gap-2 rounded-md bg-emerald-500/15 px-3 py-2 text-sm text-emerald-600 dark:text-emerald-400 font-medium">
            <CheckCircle2 className="h-3.5 w-3.5" />
            Checkpoint confirmed on-chain
          </div>
        )}
```

```tsx
        {duty && (
          <div className="flex items-center gap-2 text-sm">
            <span
              className={`h-2 w-2 rounded-full ${duty.ready ? "bg-emerald-500" : "bg-amber-500"}`}
            />
            <span className="text-muted-foreground">
              Provider duty: {duty.ready ? "Ready" : "Pending"}
            </span>
          </div>
        )}
```

```tsx
          <Button
            data-testid="trigger-checkpoint"
            variant="outline"
            size="sm"
            onClick={handleTrigger}
            disabled={loading || busy || bucketId === null}
          >
            {busy ? (
              <Loader2 className="mr-2 h-3.5 w-3.5 animate-spin" />
            ) : (
              <Shield className="mr-2 h-3.5 w-3.5" />
            )}
            {busy ? "Processing..." : "Trigger Checkpoint"}
          </Button>
```

### user-interfaces/s3-ui/src/components/CheckpointPanel.tsx (addendum)

The `toast` import (`import { toast } from "@/components/ui/toaster";`) was also removed — its only use was the removed `handleTrigger`.

### user-interfaces/drive-ui/src/lib/drive-client.ts

`CheckpointDuty` removed from both the `export type { ... } from "@web3-storage/sdk/fs"` list and the matching `import type { ... }` list.

Methods on `DriveClient`:

```ts
  getCheckpointDuty(bucketId: bigint): Promise<CheckpointDuty | null> {
    return this.requireFs().getCheckpointDuty(bucketId);
  }

  triggerCheckpoint(bucketId: bigint): Promise<void> {
    return this.requireFs().triggerCheckpoint(bucketId);
  }
```

### user-interfaces/drive-ui/src/state/checkpoint.state.ts

Removed duty state + trigger action (header comment was "…checkpoint info + provider duty status…"; the `CheckpointDuty` type import went too):

```ts
const checkpointDuty$ = new BehaviorSubject<CheckpointDuty | null>(null);

export const [useCheckpointDuty] = bind(checkpointDuty$, null);
```

Inside `refreshCheckpoint`, the duty half of the parallel fetch:

```ts
    const [info, duty] = await Promise.all([
      client.getCheckpointInfo(bucketId).catch(() => null),
      client.getCheckpointDuty(bucketId).catch(() => null),
    ]);
    checkpointInfo$.next(info);
    checkpointDuty$.next(duty);
```

```ts
export async function triggerCheckpoint(bucketId: bigint): Promise<void> {
  const client = getDriveClient();
  await client.triggerCheckpoint(bucketId);
  // Provider needs a moment to process; refresh shortly after
  setTimeout(() => {
    refreshCheckpoint(bucketId).catch(() => { /* swallow */ });
  }, 5000);
}
```

`clearCheckpointState` also lost its `checkpointDuty$.next(null)` line, and `refreshCheckpoint`'s null-bucket branch its `checkpointDuty$.next(null)`.

### user-interfaces/drive-ui/src/state/index.ts

Removed from the checkpoint export block: `useCheckpointDuty`, `triggerCheckpoint`.

### user-interfaces/drive-ui/src/components/CheckpointPanel.tsx

Removed duty display + trigger button/handler (the `useCheckpointDuty`/`triggerCheckpoint`/`toast`/`Loader2` imports went with them):

```tsx
  const duty = useCheckpointDuty();
```

```tsx
  const handleTrigger = async () => {
    try {
      await triggerCheckpoint(drive.bucketId);
      toast({ title: "Checkpoint triggered", description: "Provider is creating a checkpoint..." });
    } catch (err) {
      toast({
        title: "Checkpoint failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };
```

```tsx
        {duty && (
          <div className="flex items-center gap-2 text-xs">
            <span
              className={`h-2 w-2 rounded-full ${
                duty.ready ? "bg-green-500" : "bg-amber-500"
              }`}
            />
            <span data-testid="checkpoint-duty-status" className="text-muted-foreground">
              Provider: {duty.leafCount} leaves, {duty.ready ? "ready" : "not ready"}
            </span>
          </div>
        )}

        <Button
          data-testid="checkpoint-trigger"
          size="sm"
          className="w-full"
          onClick={handleTrigger}
          disabled={loading || (duty !== null && !duty.ready)}
        >
          {loading ? (
            <Loader2 className="mr-2 h-3 w-3 animate-spin" />
          ) : (
            <Shield className="mr-2 h-3 w-3" />
          )}
          Checkpoint Now
        </Button>
```

### examples/papi/package.json

Removed script:

```json
    "demo:checkpoint-missed": "node --import tsx checkpoint-missed.ts",
```

### user-interfaces/README.md

`checkpoint-trigger` removed from the drive-ui test-id examples line (was "- `checkpoint-panel`, `checkpoint-trigger`, `checkpoint-refresh`").

