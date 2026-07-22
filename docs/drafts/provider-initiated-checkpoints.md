# Provider-Initiated Checkpoints Design

> ⚠️ **Needs triage — [#306](https://github.com/paritytech/web3-storage/issues/306).**
> This design was added and implemented without review, and is now questioned
> (it may be redundant with replica nodes + client-held signatures). Its design
> was removed from the review-gated `docs/design/` in #305. Pending #306, this
> doc is either reviewed and canonicalized, or removed together with the related
> code. The "Problem Statement" / "Why" reasoning below has **not** been
> validated — treat with skepticism.

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
