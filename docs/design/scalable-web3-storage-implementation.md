# Scalable Web3 Storage: Implementation Details

## Overview

This document specifies the on-chain and off-chain interfaces for the storage system described in [Scalable Web3 Storage](./scalable-web3-storage.md).

---

## Bucket Semantics

A **bucket** is the fundamental unit of storage organization. It defines:

1. **Logical container**: What data belongs together
2. **Membership**: Who can read, write, or administer
3. **Canonical state**: The MMR (Merkle Mountain Range) tracking bucket contents
4. **Physical storage**: Which providers store this data (via storage agreements)

### Key Properties

**Per-bucket MMR**: The bucket has ONE canonical MMR state. Multiple providers may store the bucket, and they should all converge to this state. The MMR is not per-provider.

**Roles** (Admin implies Writer implies Reader—each can do everything the next can):
- **Admin**: Can modify members, manage settings, delete data (if not frozen)
- **Writer**: Can append data (and read)
- **Reader**: Read-only. Only meaningful on a **private** bucket, where it grants
  read access without write. Membership is the read access list for private
  buckets; see **Visibility** below.

**Visibility**: A bucket is `Public` or `Private`. On a private bucket, primaries
serve reads only to members; on a public bucket, to anyone. This is a cooperative
request to honest primaries, **not enforced on-chain**, and it does not constrain
replicas (which always serve everyone). Its single on-chain effect: on a
`Private` bucket, only members and primary-agreement owners may challenge
primaries (replicas stay challengeable by anyone).

**Redundancy**: A bucket can have storage agreements with multiple providers. The `min_providers` setting controls how many providers must acknowledge a state before it can be checkpointed. This ensures minimum redundancy for critical data.

**Append-only mode**: When `frozen_start_seq` is set, the bucket becomes append-only from that point. The start_seq can never decrease below the frozen value, preventing deletion of historical data. This is irreversible and requires the current snapshot to meet `min_providers` threshold.

### Storage Model

**Upload and Commit are separate operations**:

1. **Upload**: Clients upload content-addressed data (chunks and internal nodes) to providers. This is just storage — no MMR involvement yet. Providers accept all uploads as long as the bucket has quota. Multiple clients can upload different data concurrently without conflicts.

2. **Commit**: A client requests the provider to add data_root(s) to the bucket's MMR. The provider signs a commitment to the new MMR state. This is when data becomes "committed" and the provider becomes liable.

3. **Checkpoint**: A client submits provider signatures to the chain, establishing canonical state. The chain records which providers acknowledged this state. Only providers in the snapshot are challengeable for this state.

**No conflict rejection**: Providers accept all uploads within quota. "Conflicts" (different clients uploading different data) are fine — the checkpoint determines which state becomes canonical.

**Pruning rule**: Non-canonical branches can only be pruned once a canonical branch exists with greater depth. A branch with range `[A, A+N)` can be pruned once canonical has range `[B, B+M)` where `B + M > A + N`. This ensures providers remain liable for any data that could still be challenged.

**Optional snapshots**: On-chain snapshots are optional. Without a snapshot:
- `challenge_offchain` works (challenger provides provider signature)
- `challenge_checkpoint` fails (nothing to challenge)
- `Superseded` defense unavailable (no canonical to compare against)
- Provider is liable for ALL signed commitments
- Conflicting forks cannot be pruned

Users who create conflicts without checkpointing waste their quota—providers must keep all signed data.

**Content-addressed storage**: Everything (chunks and internal nodes) is addressed by hash. Internal nodes are data whose content is child hashes. Upload is bottom-up: children must exist before parent can be stored. If a root hash exists, the entire tree is guaranteed complete.

### Provider Lifecycle in Bucket

**Adding a provider:**
1. Admin calls `request_primary_agreement` with the provider
2. Provider calls `accept_agreement` → `StorageAgreement` created, added to `bucket.primary_providers`
3. Client uploads data to provider
4. Client requests commit, provider signs → client has provider signature
5. Client calls `checkpoint` with provider signature → provider added to `snapshot.primary_signers` bitfield

**Adding a replica provider (optional, permissionless):**
1. Anyone calls `request_agreement` with the provider and sync_balance
2. Provider calls `accept_agreement` → `StorageAgreement` created with `ProviderRole::Replica`
3. Replica syncs data autonomously from primaries, other replicas, or any data
   holder willing to push it (everything is content-addressed and self-verifying)
4. Replica calls `confirm_replica_sync` on-chain → receives per-sync payment, becomes challengeable

**Binding contract:**

Once accepted, agreements are binding for both parties until expiry:
- **No early exit for providers**: Providers cannot voluntarily leave. They committed to store data for the agreed duration.
- **No early cancellation for clients**: Clients cannot cancel and reclaim locked payment. They committed to pay for the agreed duration.
- **Provider's protection**: Before accepting, providers can set `max_duration` and review the terms. They can also block future extensions via `set_extensions_blocked`.
- **Client's protection**: Clients can challenge if provider loses data (slashing). At settlement, clients can burn payment to signal poor service.

**Agreement expiry:**

When `expires_at` is reached:
1. Provider calls `claim_expired_agreement` to receive payment, OR
2. Client calls `end_agreement` with pay/burn decision within settlement window
3. Provider is no longer bound to store data
4. Provider won't be included in future checkpoints

**Snapshot liability**: Providers remain liable for snapshots they signed until those snapshots are superseded by a new checkpoint that doesn't include them, or until the bucket's canonical depth grows past the data they signed for.

### Multi-Provider Coordination (Primary Providers)

Primary providers don't sync with each other. Clients are responsible for uploading to each primary provider they want to store their data.

**Flow**:
1. Client uploads data to Primary A, B, C (separately)
2. Client triggers commit on each provider, collects signatures
3. Client checkpoints on-chain with collected signatures
4. Primaries not in the snapshot should sync (client re-uploads)
5. After checkpoint, providers can prune non-canonical roots

**Liability**: A provider is only liable for MMR states they acknowledged (signed). Challenges against the canonical checkpoint only work for providers listed in the snapshot's provider bitfield.

**Replica providers** sync autonomously from primaries or other replicas. They confirm sync on-chain and are liable for the roots they've confirmed.

---

## On-Chain: Pallet Interface

### The anchor clock (block-number denomination)

Every duration, deadline and timeout in this pallet is measured against the
**anchor block** — sourced from `Config::BlockNumberProvider` (the relay chain in
production) — not the parachain block height, so wall-clock durations stay stable
when the parachain block time changes.

Read it via `Pallet::current_anchor_block()` on-chain, or the
`current_anchor_block` / `anchor_block_time_millis` runtime APIs off-chain — never
a raw `frame_system::block_number()`, which is the parachain height and unrelated
to the anchor on any network where the clocks differ. Pseudocode below that still
shows `frame_system::block_number()` is illustrative; the implementation uses the
anchor.

### Pallet Config

```rust
#[pallet::config]
pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
    /// Currency type for payments and staking.
    type Currency: ReservableCurrency<Self::AccountId>;

    /// Treasury account to receive burned payments.
    type Treasury: Get<Self::AccountId>;

    /// Minimum stake per byte committed (e.g., 1 token per GB = 1e12 per 1e9 bytes).
    /// Prevents providers from over-committing relative to their collateral.
    #[pallet::constant]
    type MinStakePerByte: Get<BalanceOf<Self>>;

    /// Maximum length of provider multiaddr.
    #[pallet::constant]
    type MaxMultiaddrLength: Get<u32>;

    /// Maximum members per bucket.
    #[pallet::constant]
    type MaxMembers: Get<u32>;

    /// Maximum primary providers per bucket (e.g., 5).
    #[pallet::constant]
    type MaxPrimaryProviders: Get<u32>;

    /// Minimum stake required to register as a provider.
    /// Governance-controlled to bound total provider count and provide sybil resistance.
    #[pallet::constant]
    type MinProviderStake: Get<BalanceOf<Self>>;

    /// Maximum chunk size for challenge responses (e.g., 256 KiB).
    #[pallet::constant]
    type MaxChunkSize: Get<u32>;

    /// Timeout for challenge response (e.g., ~48 hours, in anchor blocks — see
    /// the anchor-clock note above).
    #[pallet::constant]
    type ChallengeTimeout: Get<BlockNumberFor<Self>>;

    /// Settlement window after agreement expiry for owner to call end_agreement.
    #[pallet::constant]
    type SettlementTimeout: Get<BlockNumberFor<Self>>;

    /// Maximum duration an agreement request can sit unanswered before it expires.
    #[pallet::constant]
    type RequestTimeout: Get<BlockNumberFor<Self>>;

    /// Maximum number of buckets a single account can be a member of
    /// (bounds the per-account reverse index).
    #[pallet::constant]
    type MaxBucketsPerMember: Get<u32>;

    /// Minimum number of blocks between announcing a deregistration and
    /// being allowed to complete it. Must be strictly `> ChallengeTimeout`
    /// so any challenge created up to the announcement block matures while
    /// the provider is still slashable, and `> RequestTimeout` so a
    /// pre-deregistration agreement quote expires before re-registration.
    #[pallet::constant]
    type DeregisterAnnouncementPeriod: Get<BlockNumberFor<Self>>;

    /// Caps the challenges sharing one deadline (anchor block) and the
    /// `on_initialize` sweep's per-block slash budget.
    #[pallet::constant]
    type MaxChallengesPerDeadline: Get<u16>;

    /// The anchor clock: source of the block number every duration and
    /// deadline above is measured against (the relay chain in production,
    /// `frame_system` in tests). Pinned to the parachain block-number type.
    type BlockNumberProvider: BlockNumberProvider<BlockNumber = SystemBlockNumberFor<Self>>;

    /// Milliseconds per anchor block (6000 for a relay-chain anchor).
    /// Exposed via the `anchor_block_time_millis` runtime API.
    #[pallet::constant]
    type AnchorBlockTimeMillis: Get<u64>;

    /// Weight information for extrinsics.
    type WeightInfo: WeightInfo;
}
```

Reference runtime values (see `runtimes/web3-storage-local/src/storage.rs`).
All durations are in anchor (relay-chain) blocks — `RC_HOURS`, not the
parachain `HOURS`:

| Constant | Value |
|---|---|
| `MinProviderStake` | `1_000 * UNIT` (1000 tokens) |
| `MinStakePerByte` | `1_000` |
| `MaxMultiaddrLength` | `128` |
| `MaxMembers` | `100` |
| `MaxPrimaryProviders` | `5` |
| `MaxChunkSize` | `262_144` (256 KiB) |
| `ChallengeTimeout` | `48 * RC_HOURS` |
| `SettlementTimeout` | `24 * RC_HOURS` |
| `RequestTimeout` | `6 * RC_HOURS` |
| `MaxBucketsPerMember` | `1_000` |
| `DeregisterAnnouncementPeriod` | `54 * RC_HOURS` (48h challenge window + 6h grace) |
| `MaxChallengesPerDeadline` | `1_000` |
| `AnchorBlockTimeMillis` | `6_000` |
| `Treasury` | derived from `PalletId(*b"py/trsry")` |

### Storage Items

```rust
/// Provider registry
#[pallet::storage]
pub type Providers<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    T::AccountId,
    ProviderInfo<T>,
>;

pub struct ProviderInfo<T: Config> {
    /// Multiaddr for connecting to this provider
    pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
    /// Public key for signature verification. Raw bytes so multiple key types
    /// are supported: 32 bytes for Sr25519/Ed25519, 33 for compressed Ecdsa,
    /// 64 reserved for future schemes.
    pub public_key: BoundedVec<u8, ConstU32<64>>,
    /// Total stake locked by this provider
    pub stake: BalanceOf<T>,
    /// Total contracted bytes (sum of max_bytes across all agreements)
    /// Used for stake/bytes ratio — represents commitment, not actual storage
    pub committed_bytes: u64,
    /// Provider settings
    pub settings: ProviderSettings<T>,
    /// Provider statistics - clients use these to evaluate quality
    pub stats: ProviderStats<T>,
    /// Block at which a previously-announced deregistration becomes
    /// finalisable via `complete_deregister`. `None` means no announcement
    /// is in progress. During the announcement window the provider is still
    /// on-chain and still slashable for any pending challenge.
    pub deregister_at: Option<BlockNumberFor<T>>,
}

/// On-chain statistics for evaluating provider quality.
/// These are objective, verifiable metrics that help clients make informed decisions.
pub struct ProviderStats<T: Config> {
    /// Block when provider registered (track provider age)
    pub registered_at: BlockNumberFor<T>,
    /// Total agreements ever created with this provider
    pub agreements_total: u32,
    /// Agreements where client chose to extend (signal of satisfaction)
    pub agreements_extended: u32,
    /// Agreements that expired without extension (neutral/negative signal)
    pub agreements_not_extended: u32,
    /// Agreements where client burned payment (strong negative signal)
    pub agreements_burned: u32,
    /// Total bytes ever committed across all agreements (historical volume)
    pub total_bytes_committed: u64,
    /// Challenges from authorized challengers (member/agreement owner at
    /// challenge creation) that the provider responded to. Counted at
    /// resolution—cancelled challenges are not counted.
    pub challenges_received_authorized: u32,
    /// Same, for general-public challengers.
    pub challenges_received_public: u32,
    /// Number of challenges where provider was slashed (critical failure).
    /// Tier-independent and disjoint from the received counters: a challenge
    /// resolves into exactly one of received_authorized / received_public
    /// (successfully defended), failed (slashed), or nothing (cancelled).
    pub challenges_failed: u32,
}

pub struct ProviderSettings<T: Config> {
    /// Minimum agreement duration provider will accept
    pub min_duration: BlockNumberFor<T>,
    /// Maximum agreement duration provider will accept
    pub max_duration: BlockNumberFor<T>,
    /// Price per byte per block for storage
    pub price_per_byte: BalanceOf<T>,
    /// Whether accepting new primary agreements
    pub accepting_primary: bool,
    /// Price per successful sync confirmation, or None if not accepting replicas.
    /// Replicas are paid this amount each time they confirm sync to a new snapshot.
    /// Covers: sync work, bandwidth costs to fetch from primaries, profit margin.
    pub replica_sync_price: Option<BalanceOf<T>>,
    /// Whether accepting extensions on existing agreements
    pub accepting_extensions: bool,
    /// Maximum storage capacity in bytes. `0` means unlimited.
    /// When non-zero, the provider cannot accept agreements that would push
    /// `committed_bytes` past this value, and the provider's stake must back
    /// it: `stake >= max_capacity * MinStakePerByte`.
    pub max_capacity: u64,
}

/// Monotonically increasing bucket ID counter. Ensures stable, unique IDs.
#[pallet::storage]
pub type NextBucketId<T: Config> = StorageValue<_, BucketId, ValueQuery>;

/// Bucket ID is a stable, unique identifier (not an index into a collection).
/// Using u64 ensures IDs never get reused even if buckets are deleted.
pub type BucketId = u64;

/// Buckets: containers for data with membership and storage agreements
#[pallet::storage]
pub type Buckets<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    BucketId,
    Bucket<T>,
>;

pub struct Member<T: Config> {
    pub account: T::AccountId,
    pub role: Role,
}

pub enum Role {
    /// Can modify members, manage settings, delete data (if not frozen).
    /// Implicitly can also read and write.
    Admin,
    /// Can append data. Implicitly can also read.
    Writer,
    /// Read-only access. Only meaningful on a private bucket: it grants reading
    /// without writing (see "Roles" / "Visibility" in Bucket Semantics).
    Reader,
}

/// Whether primaries serve reads to anyone, or only to members.
pub enum Visibility {
    /// Primaries serve reads to anyone.
    Public,
    /// Primaries serve reads only to members (Admin/Writer/Reader). This
    /// members-only restriction is a cooperative request to honest primaries,
    /// not on-chain-enforced; replicas serve everyone regardless. On-chain,
    /// `Private` restricts primary challenges to members and primary-agreement
    /// owners. Full semantics: design doc, "Bucket Visibility & Access".
    Private,
}

pub struct Bucket<T: Config> {
    /// Members who can interact with this bucket
    pub members: BoundedVec<Member<T>, T::MaxMembers>,
    /// Read visibility (see `Visibility`). On-chain, only the challenge
    /// extrinsics read it: `Private` restricts primary challenges to members
    /// and primary-agreement owners.
    pub visibility: Visibility,
    /// If Some, bucket is append-only from this start_seq.
    /// Checkpoints with start_seq < frozen_start_seq are rejected (prevents deletions).
    pub frozen_start_seq: Option<u64>,
    /// Minimum primary provider signatures required for checkpoint.
    pub min_providers: u32,
    /// Primary provider account IDs (limited to T::MaxPrimaryProviders, e.g., 5).
    /// These are admin-controlled providers that:
    /// - Receive data directly from writers
    /// - Count toward min_providers for checkpoints
    /// - Can be early-terminated by admin (with pay/burn)
    /// Stored inline for efficient checkpoint reads (one storage access).
    pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
    /// Current canonical state
    pub snapshot: Option<BucketSnapshot<T>>,
    /// Historical MMR roots for replica sync validation.
    /// 
    /// **Why we need this:**
    /// Replicas sync autonomously and may lag behind the current snapshot. When a
    /// replica confirms sync, we need to verify they actually synced to a valid
    /// historical state (not a fabricated root). But storing every historical root
    /// would be unbounded. Prime-based bucketing gives us O(1) storage with
    /// logarithmic time coverage - a replica that's 100 blocks behind can still
    /// prove sync to a valid root, while ancient roots naturally age out.
    /// 
    /// **How it works:**
    /// Uses prime-based bucketing for logarithmic time coverage:
    /// Position 0: updated every 3 blocks (prime = 3)
    /// Position 1: updated every 7 blocks (prime = 7)
    /// Position 2: updated every 11 blocks (prime = 11)
    /// Position 3: updated every 23 blocks (prime = 23)
    /// Position 4: updated every 47 blocks (prime = 47)
    /// Position 5: updated every 113 blocks (prime = 113)
    /// 
    /// Each entry stores (quotient, mmr_root) where quotient = block_number / prime.
    /// On each checkpoint, if current_block / prime != stored quotient, the entry
    /// is updated with (new_quotient, current_snapshot_root). This means each
    /// position remembers the root from the last time its prime boundary was crossed.
    /// 
    /// Primes ensure positions don't align, maximizing coverage. A slow replica
    /// can match against older positions; `position_matched` in events tracks this.
    pub historical_roots: [(u32, H256); 6],
    /// Total snapshots created for this bucket (for statistics)
    pub total_snapshots: u32,
}

pub struct BucketSnapshot<BlockNumber> {
    /// Canonical MMR commitment at this checkpoint
    pub commitment: Commitment,
    /// Block at which checkpointed
    pub checkpoint_block: BlockNumber,
    /// Bitfield indicating which primary providers signed this snapshot.
    /// Bit i (LSB0) is set if `primary_providers[i]` signed.
    /// Stored as `Vec<u8>` with explicit `count_signers()` / `has_provider_signed()`
    /// helpers rather than `BitVec` to keep encoding stable and `no_std`-friendly.
    /// `primary_providers` is bounded by `T::MaxPrimaryProviders` (e.g., 5), so
    /// indices are stable within a checkpoint; if it changes between checkpoints
    /// the bitfield is regenerated at the next checkpoint.
    pub primary_signers: Vec<u8>,
    /// The `nonce` value from the `CommitmentPayload` that the original
    /// signers signed. Required by `extend_checkpoint` so a late-arriving
    /// signature can be verified against the same payload the initial
    /// signers committed to.
    pub commitment_nonce: u64,
}
// Canonical range is [start_seq, start_seq + leaf_count)
// Destructive writes (new MMR that allows pruning old) must set start_seq >= old_start_seq + old_leaf_count

/// Storage agreements: per-provider contracts for a bucket
#[pallet::storage]
pub type StorageAgreements<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat,
    BucketId,
    Blake2_128Concat,
    T::AccountId,
    StorageAgreement<T>,
>;

pub struct StorageAgreement<T: Config> {
    /// Who owns this agreement (can top up quota, transfer ownership)
    pub owner: T::AccountId,
    /// Maximum bytes (quota) — provider accepts uploads up to this
    pub max_bytes: u64,
    /// Payment locked for storage (bytes * time)
    pub payment_locked: BalanceOf<T>,
    /// Price per byte locked at creation/last extension.
    /// Used to determine if extension requires owner approval (price increases).
    pub price_per_byte: BalanceOf<T>,
    /// Agreement expiration
    pub expires_at: BlockNumberFor<T>,
    /// Whether provider has blocked extensions for this specific agreement
    pub extensions_blocked: bool,
    /// Provider role for this bucket.
    pub role: ProviderRole<T>,
    /// Block when agreement became active (for statistics)
    pub started_at: BlockNumberFor<T>,
}

#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub enum ProviderRole<T: Config> {
    /// Receives data directly from writers.
    /// - Admin-controlled (stored in bucket.primary_providers)
    /// - Count toward min_providers for checkpoints
    /// - Can be early-terminated by admin
    Primary,
    /// Syncs data from other providers autonomously.
    /// - Permissionless (anyone can add)
    /// - Does NOT count toward min_providers
    /// - Cannot be early-terminated (runs to expiry)
    /// - Receives per-sync payment from sync_balance
    Replica {
        /// Balance for per-sync payments (drawn down on each sync confirmation)
        sync_balance: BalanceOf<T>,
        /// Price per sync locked at creation/last extension
        sync_price: BalanceOf<T>,
        /// Minimum blocks between sync confirmations for this agreement.
        /// Set at agreement creation based on expected bucket activity.
        /// 0 means no time-based limit (only "new root" check applies).
        min_sync_interval: BlockNumberFor<T>,
        /// Last confirmed sync: (mmr_root, block_number).
        /// None if replica hasn't confirmed sync yet.
        last_sync: Option<(H256, BlockNumberFor<T>)>,
    },
}

/// Pending agreement requests (client → provider, awaiting acceptance)
/// Keyed by (provider, bucket) so providers can efficiently query their pending requests
#[pallet::storage]
pub type AgreementRequests<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat,
    T::AccountId,  // provider (first key for efficient provider queries)
    Blake2_128Concat,
    BucketId,
    AgreementRequest<T>,
>;

pub struct AgreementRequest<T: Config> {
    /// Who requested the agreement
    pub requester: T::AccountId,
    /// Maximum bytes requested
    pub max_bytes: u64,
    /// Payment locked by requester
    pub payment_locked: BalanceOf<T>,
    /// Requested duration
    pub duration: BlockNumberFor<T>,
    /// Block at which request expires if not accepted/rejected
    pub expires_at: BlockNumberFor<T>,
    /// Replica-specific parameters, None for primary agreements.
    /// Presence distinguishes the agreement type at request time.
    pub replica_params: Option<ReplicaRequestParams<T>>,
}

/// Parameters specific to replica agreement requests.
pub struct ReplicaRequestParams<T: Config> {
    /// Initial sync balance to fund per-sync payments
    pub sync_balance: BalanceOf<T>,
    /// Minimum blocks between sync confirmations.
    /// 0 means no time-based limit (only "new root" check applies).
    pub min_sync_interval: BlockNumberFor<T>,
}

/// Pending challenges, keyed by (deadline anchor block, per-deadline index).
/// At most `MaxChallengesPerDeadline` challenges share a deadline; expired
/// deadlines are drained by the `on_initialize` slash sweep.
#[pallet::storage]
pub type Challenges<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, BlockNumberFor<T>, // deadline (anchor block)
    Blake2_128Concat, u16,               // index within the deadline
    Challenge<T>,
>;

/// Per-deadline index allocator for `Challenges` (monotone; never reused
/// within a deadline, cleared when the sweep drains the deadline).
#[pallet::storage]
pub type NextChallengeIndex<T: Config> =
    StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, u16, ValueQuery>;

/// Cursor of the `on_initialize` slash sweep: every deadline up to and
/// including this anchor block has been drained. Each block the sweep
/// advances it toward the current anchor (exclusive), slashing expired
/// challenges as it goes, capped per block by a span and slash budget.
#[pallet::storage]
pub type LastSweptChallengeBlock<T: Config> =
    StorageValue<_, BlockNumberFor<T>, OptionQuery>;

/// Challenge identifier combining deadline and index.
/// Challenges are stored by deadline block for efficient expiry processing.
/// Defined in `storage_primitives`, generic over `BlockNumber`.
pub struct ChallengeId<BlockNumber> {
    /// Block by which provider must respond
    pub deadline: BlockNumber,
    /// Index within the deadline's challenge list
    pub index: u16,
}

pub struct Challenge<T: Config> {
    /// Bucket containing the challenged data
    pub bucket_id: BucketId,
    /// Provider being challenged
    pub provider: T::AccountId,
    /// Account that issued the challenge
    pub challenger: T::AccountId,
    /// MMR root the provider committed to
    pub mmr_root: H256,
    /// Start sequence of the commitment (needed to compute challenged_seq = start_seq + target.leaf_index)
    pub start_seq: u64,
    /// Leaf + chunk being challenged (see `ChunkLocation`)
    pub target: ChunkLocation,
    /// Deposit locked by the challenger when the challenge was created.
    /// Returned (in part) on successful defense, forfeited on invalid challenge,
    /// refunded in full — with no reward — if the provider is slashed (the
    /// slash goes to the Treasury; see "no reward beyond actual costs").
    pub deposit: BalanceOf<T>,
    /// Whether the challenger was authorized (bucket member or agreement owner,
    /// via `is_authorized`) at challenge creation. Snapshotted here so
    /// membership/agreement changes between creation and response cannot alter
    /// the fee split applied in `respond_to_challenge`.
    pub authorized: bool,
}

/// Reverse index: account → bucket IDs they are a member of.
/// Bounded by `T::MaxBucketsPerMember` to keep iteration costs predictable.
#[pallet::storage]
pub type MemberBuckets<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    T::AccountId,
    BoundedVec<BucketId, T::MaxBucketsPerMember>,
    ValueQuery,
>;
```

### Provider Public Key & Signature Type

Providers register a raw public key alongside their multiaddr (32 bytes for
Sr25519/Ed25519, 33 bytes for compressed Ecdsa). All on-chain signature
verification uses `sp_runtime::MultiSignature` against this key, so a single
provider can use any of the supported schemes.

> **⚠️ Scheme asymmetry — [#274](https://github.com/paritytech/web3-storage/issues/274).**
> On-chain verification is multi-scheme (`MultiSignature`:
> Sr25519/Ed25519/Ecdsa), but the provider node's off-chain HTTP auth (see
> [Authentication & RBAC](#authentication--rbac)) verifies **sr25519 only**, and
> only sr25519 is exercised end-to-end. The supported matrix (incl. the reserved
> 64-byte / `Eth` shapes the pallet accepts but can't verify) is **unratified** —
> this doc should be updated once #274 decides it.

Two on-chain signed payloads exist (all SCALE-encoded, all carry an explicit
`version: u8` so the protocol can evolve without breaking existing signatures):

- `CommitmentPayload { version, bucket_id, commitment, nonce }` — what
  providers sign for `commit`, `checkpoint`, `extend_checkpoint`, and
  `challenge_offchain` (`commitment: Commitment` is defined in [Data
  Structures](#data-structures)). For `challenge_offchain` the challenger
  passes the signed `commitment` through unchanged so the pallet's payload
  reconstruction matches the signature.
- The replica sync `roots` array (`[Option<H256>; 7]`) — signed for
  `confirm_replica_sync` to attest which roots the replica actually has.

### Events

```rust
#[pallet::event]
pub enum Event<T: Config> {
    // ─────────────────────────────────────────────────────────────
    // Provider events
    // ─────────────────────────────────────────────────────────────
    
    ProviderRegistered {
        provider: T::AccountId,
        stake: BalanceOf<T>,
    },
    /// Final deregistration: stake returned, provider entry removed.
    ProviderDeregistered {
        provider: T::AccountId,
        stake_returned: BalanceOf<T>,
    },
    /// First step of the two-step exit — provider declared intent to leave.
    /// Stake stays reserved and they remain slashable until `complete_after`.
    DeregisterAnnounced {
        provider: T::AccountId,
        complete_after: BlockNumberFor<T>,
    },
    /// Provider cancelled their announced deregistration before the window elapsed.
    DeregisterCancelled {
        provider: T::AccountId,
    },
    ProviderStakeAdded {
        provider: T::AccountId,
        amount: BalanceOf<T>,
        total_stake: BalanceOf<T>,
    },
    ProviderSettingsUpdated {
        provider: T::AccountId,
        settings: ProviderSettings<T>,
    },
    ProviderMultiaddrUpdated {
        provider: T::AccountId,
    },
    ExtensionsBlocked {
        bucket_id: BucketId,
        provider: T::AccountId,
        blocked: bool,
    },

    // ─────────────────────────────────────────────────────────────
    // Bucket events
    // ─────────────────────────────────────────────────────────────
    
    BucketCreated {
        bucket_id: BucketId,
        admin: T::AccountId,
    },
    BucketFrozen {
        bucket_id: BucketId,
        frozen_start_seq: u64,
    },
    BucketDeleted {
        bucket_id: BucketId,
    },
    MemberSet {
        bucket_id: BucketId,
        member: T::AccountId,
        role: Role,
    },
    MemberRemoved {
        bucket_id: BucketId,
        member: T::AccountId,
    },
    BucketCheckpointed {
        bucket_id: BucketId,
        commitment: Commitment,
        providers: Vec<T::AccountId>,
    },
    ProviderAddedToBucket {
        bucket_id: BucketId,
        provider: T::AccountId,
    },
    PrimaryProviderRemoved {
        bucket_id: BucketId,
        provider: T::AccountId,
        reason: RemovalReason,
    },
    PrimaryAgreementEndedEarly {
        bucket_id: BucketId,
        provider: T::AccountId,
        payment_to_provider: BalanceOf<T>,
        burned: BalanceOf<T>,
    },
    SlashedProviderRemoved {
        bucket_id: BucketId,
        provider: T::AccountId,
        payment_returned_to_owner: BalanceOf<T>,
    },

    // ─────────────────────────────────────────────────────────────
    // Replica events
    // ─────────────────────────────────────────────────────────────

    /// Emitted when a replica confirms sync to a snapshot.
    /// position_matched indicates sync latency:
    /// - 0 = current snapshot (excellent)
    /// - 1-6 = historical positions [base3, base7, base11, base23, base47, base113]
    /// Higher positions indicate the replica is syncing to older snapshots.
    ReplicaSynced {
        bucket_id: BucketId,
        provider: T::AccountId,
        mmr_root: H256,
        position_matched: u8,
        sync_payment: BalanceOf<T>,
    },
    ReplicaSyncBalanceToppedUp {
        bucket_id: BucketId,
        provider: T::AccountId,
        amount: BalanceOf<T>,
        new_total: BalanceOf<T>,
    },

    // ─────────────────────────────────────────────────────────────
    // Agreement events
    // ─────────────────────────────────────────────────────────────
    
    AgreementRequested {
        bucket_id: BucketId,
        provider: T::AccountId,
        requester: T::AccountId,
        max_bytes: u64,
        payment_locked: BalanceOf<T>,
        duration: BlockNumberFor<T>,
    },
    AgreementAccepted {
        bucket_id: BucketId,
        provider: T::AccountId,
        expires_at: BlockNumberFor<T>,
    },
    AgreementRejected {
        bucket_id: BucketId,
        provider: T::AccountId,
        payment_returned: BalanceOf<T>,
    },
    AgreementRequestWithdrawn {
        bucket_id: BucketId,
        provider: T::AccountId,
        payment_returned: BalanceOf<T>,
    },
    AgreementToppedUp {
        bucket_id: BucketId,
        provider: T::AccountId,
        amount: BalanceOf<T>,
        new_max_bytes: u64,
    },
    AgreementExtended {
        bucket_id: BucketId,
        provider: T::AccountId,
        new_expires_at: BlockNumberFor<T>,
        payment: BalanceOf<T>,
    },
    AgreementOwnershipTransferred {
        bucket_id: BucketId,
        provider: T::AccountId,
        old_owner: T::AccountId,
        new_owner: T::AccountId,
    },
    AgreementEnded {
        bucket_id: BucketId,
        provider: T::AccountId,
        payment_to_provider: BalanceOf<T>,
        burned: BalanceOf<T>,
    },
    AgreementExpiredClaimed {
        bucket_id: BucketId,
        provider: T::AccountId,
        payment_to_provider: BalanceOf<T>,
    },

    // ─────────────────────────────────────────────────────────────
    // Challenge events
    // ─────────────────────────────────────────────────────────────
    
    /// A challenge was issued against a provider
    ChallengeCreated {
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        bucket_id: BucketId,
        provider: T::AccountId,
        challenger: T::AccountId,
        respond_by: BlockNumberFor<T>,
    },
    /// Provider responded successfully to a challenge.
    /// `provider_cost` is the fraction of the response tx fee the provider
    /// bears itself (paid from its account, never its stake): a share per the
    /// cost-split table for authorized challengers, and always 0 for public
    /// challengers (who fund the provider's fee in full). `challenger_cost` is
    /// what the challenger's deposit ultimately funded; any excess deposit is
    /// returned.
    ChallengeDefended {
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        provider: T::AccountId,
        response_time_blocks: BlockNumberFor<T>,
        challenger_cost: BalanceOf<T>,
        provider_cost: BalanceOf<T>,
    },
    /// Provider failed to respond or provided invalid proof - slashed
    ChallengeSlashed {
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        provider: T::AccountId,
        slashed_amount: BalanceOf<T>,
    },

}
```

### Runtime API

Read-only queries used by clients (the Rust SDK, demos, and the provider
node's membership cache) to discover providers, inspect bucket state, list
agreements, and watch challenges without submitting transactions. Defined in
`crates/pallets/storage-provider/src/runtime_api.rs` as `StorageProviderApi`.

```rust
sp_api::decl_runtime_apis! {
    pub trait StorageProviderApi<AccountId, BlockNumber, Balance>
    where
        AccountId: Encode + Decode,
        BlockNumber: Encode + Decode,
        Balance: Encode + Decode,
    {
        // ── Provider directory ────────────────────────────────────────────
        /// Provider info for a single account.
        fn provider_info(provider: AccountId) -> Option<ProviderInfoResponse>;
        /// Paginated list of all registered providers.
        fn providers(offset: u32, limit: u32) -> Vec<(AccountId, ProviderInfoResponse)>;
        /// Providers with at least `bytes_needed` of available capacity.
        fn providers_with_capacity(
            bytes_needed: u64,
            offset: u32,
            limit: u32,
        ) -> Vec<(AccountId, ProviderInfoResponse)>;
        /// Find providers matching given requirements, sorted by match score
        /// (best first). Used by the SDK's discovery client.
        fn find_matching_providers(
            requirements: StorageRequirements,
            limit: u32,
        ) -> Vec<MatchedProvider>;
        /// Quick check: does the provider's stake/capacity support an extra `additional_bytes`?
        fn can_accept_bytes(provider: AccountId, additional_bytes: u64) -> bool;

        // ── Buckets ───────────────────────────────────────────────────────
        fn bucket_info(bucket_id: BucketId) -> Option<BucketResponse>;
        fn bucket_ids(offset: u32, limit: u32) -> Vec<BucketId>;
        fn bucket_providers(bucket_id: BucketId) -> Vec<AccountId>;

        // ── Agreements ────────────────────────────────────────────────────
        fn agreement_info(bucket_id: BucketId, provider: AccountId) -> Option<AgreementResponse>;
        fn bucket_agreements(bucket_id: BucketId) -> Vec<AgreementResponse>;
        fn provider_agreements(provider: AccountId) -> Vec<AgreementResponse>;

        // ── Challenges ────────────────────────────────────────────────────
        fn challenges_at(block: BlockNumber) -> Vec<ChallengeResponse>;
        fn bucket_challenges(bucket_id: BucketId) -> Vec<ChallengeResponse>;
        fn provider_challenges(provider: AccountId) -> Vec<ChallengeResponse>;
        fn challenger_challenges(challenger: AccountId) -> Vec<ChallengeResponse>;
    }
}
```

Response types live in `crates/pallets/storage-provider/src/runtime_api.rs` (`ProviderInfoResponse`,
`StorageRequirements`, `MatchedProvider`, `BucketResponse`,
`AgreementResponse`, `ChallengeResponse`, etc.). They flatten the on-chain
structs into encode/decode-friendly shapes (e.g. `AccountId` as `Vec<u8>`,
`Balance` as `u128`) so client-side SDKs don't need to depend on the runtime's
generics. `MatchedProvider` also carries a `match_score` (0–100) and an
optional `PartialMatchReason` (price, capacity, duration, not-accepting) for
the marketplace UI to surface why a provider didn't qualify.

### Extrinsics

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    // ─────────────────────────────────────────────────────────────
    // Provider management
    // ─────────────────────────────────────────────────────────────

    /// Register as a storage provider.
    /// 
    /// Creates a new provider entry with the given multiaddr, public key, and
    /// initial stake. Stake must be at least `T::MinProviderStake`.
    /// 
    /// Parameters:
    /// - `multiaddr`: Network address where clients can connect to this provider
    /// - `public_key`: Raw public key bytes — 32 for Sr25519/Ed25519, 33 for
    ///   compressed Ecdsa, 64 reserved. Used to verify provider signatures
    ///   (commitments, checkpoints, replica sync) on-chain.
    /// - `stake`: Initial stake to lock (must meet minimum, provides sybil resistance)
    #[pallet::weight(...)]
    pub fn register_provider(
        origin: OriginFor<T>,
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        public_key: BoundedVec<u8, ConstU32<64>>,
        stake: BalanceOf<T>,
    ) -> DispatchResult;

    /// Add stake to an existing provider registration.
    /// 
    /// Stake can only increase; to withdraw stake, use `deregister_provider`.
    /// Higher stake improves stake/bytes ratio, allowing more agreements.
    /// 
    /// Parameters:
    /// - `amount`: Additional stake to lock
    #[pallet::weight(...)]
    pub fn add_stake(
        origin: OriginFor<T>,
        amount: BalanceOf<T>,
    ) -> DispatchResult;

    /// Announce intent to deregister (step 1 of 2).
    ///
    /// Stamps `deregister_at = now + T::DeregisterAnnouncementPeriod`, freezes
    /// `accepting_primary` / `accepting_extensions` to `false`, and keeps the
    /// stake reserved. The provider remains on-chain and fully slashable for
    /// any challenge created up to the announcement block.
    ///
    /// Fails if `committed_bytes > 0`: providers must let active agreements
    /// expire first. The two-step flow closes the race where a provider could
    /// withdraw stake between the end of their last agreement and a
    /// freshly-created challenge.
    #[pallet::weight(...)]
    pub fn deregister_provider(origin: OriginFor<T>) -> DispatchResult;

    /// Finalise a previously-announced deregistration (step 2 of 2).
    ///
    /// Callable once `T::DeregisterAnnouncementPeriod` has elapsed since
    /// `deregister_provider`. Unreserves the remaining stake and removes the
    /// provider record. Still requires `committed_bytes == 0`.
    #[pallet::weight(...)]
    pub fn complete_deregister(origin: OriginFor<T>) -> DispatchResult;

    /// Cancel a previously-announced deregistration before the window elapses.
    ///
    /// Restores `accepting_primary` / `accepting_extensions` to `true`
    /// (mirroring what `deregister_provider` forced to `false` on announce)
    /// and clears `deregister_at`. The provider can update settings afterwards.
    #[pallet::weight(...)]
    pub fn cancel_deregister(origin: OriginFor<T>) -> DispatchResult;

    /// Update provider settings.
    /// 
    /// Allows provider to change pricing, duration limits, capacity, and
    /// availability. Changes apply to new agreements only; existing agreements
    /// retain their locked terms.
    ///
    /// Validation:
    /// - `min_duration <= max_duration` (`MinDurationExceedsMaxDuration`).
    /// - If `max_capacity > 0`: must be `>= committed_bytes`
    ///   (`CapacityBelowCommitted`) and stake must cover it,
    ///   i.e. `stake >= max_capacity * MinStakePerByte`
    ///   (`InsufficientStakeForCapacity`).
    /// - Settings are frozen while a deregister announcement is in flight —
    ///   call `cancel_deregister` first.
    /// 
    /// Parameters:
    /// - `settings`: New provider settings (pricing, duration, capacity, accepting flags)
    #[pallet::weight(...)]
    pub fn update_provider_settings(
        origin: OriginFor<T>,
        settings: ProviderSettings<T>,
    ) -> DispatchResult;

    /// Update only the provider's multiaddr (network endpoint).
    ///
    /// Cheaper and narrower than `update_provider_settings` for a common case:
    /// the provider physically moved hosts but everything else (pricing,
    /// capacity, accepting flags) stays the same.
    #[pallet::weight(...)]
    pub fn update_provider_multiaddr(
        origin: OriginFor<T>,
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
    ) -> DispatchResult;

    /// Block or unblock extensions for a specific bucket (provider only).
    /// Allows provider to stop a specific bucket from extending while
    /// continuing to accept extensions from other buckets.
    #[pallet::weight(...)]
    pub fn set_extensions_blocked(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        blocked: bool,
    ) -> DispatchResult;




    // ─────────────────────────────────────────────────────────────
    // Bucket management
    // ─────────────────────────────────────────────────────────────

    /// Create a new bucket.
    /// 
    /// The caller becomes the bucket admin. The bucket starts empty with no
    /// providers or data.
    /// 
    /// Parameters:
    /// - `min_providers`: Minimum primary provider signatures required for checkpoints
    /// - `visibility`: `Public` or `Private` (see `Visibility`). Wrappers that
    ///   omit the choice must default to `Private` (fail-safe: an unset choice
    ///   should protect data, not expose it).
    #[pallet::weight(...)]
    pub fn create_bucket(
        origin: OriginFor<T>,
        min_providers: u32,
        visibility: Visibility,
    ) -> DispatchResult;

    /// Create a bucket and an agreement with an auto-selected provider in one call.
    ///
    /// Convenience extrinsic that:
    /// 1. Runs `find_matching_provider(max_bytes, duration, max_price_per_byte)`
    ///    against on-chain provider settings.
    /// 2. Creates a bucket with `min_providers = 1` and the matched provider
    ///    pushed straight into `primary_providers` (no pending request flow).
    /// 3. Reserves `provider.price_per_byte * max_bytes * duration` from the
    ///    caller as locked payment.
    ///
    /// Providers who set `accepting_primary: true` have pre-consented to
    /// agreements within their advertised parameters, so no acceptance step
    /// is needed. Fails with `NoMatchingProvider` if nothing fits.
    #[pallet::weight(...)]
    pub fn create_bucket_with_storage(
        origin: OriginFor<T>,
        max_bytes: u64,
        duration: BlockNumberFor<T>,
        max_price_per_byte: BalanceOf<T>,
        visibility: Visibility,
    ) -> DispatchResult;

    /// Set minimum providers required for checkpoint (admin only).
    /// 
    /// Controls redundancy: checkpoints require at least this many primary provider
    /// signatures to be accepted. Cannot exceed current primary provider count.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket to modify
    /// - `min_providers`: New minimum provider count for checkpoints
    #[pallet::weight(...)]
    pub fn set_min_providers(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        min_providers: u32,
    ) -> DispatchResult;

    /// Freeze bucket — make append-only (admin only, irreversible)
    /// Requires snapshot with min_providers acknowledgments
    pub fn freeze_bucket(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult;

    /// Set bucket read visibility (admin only).
    ///
    /// Flips `Public` ⇄ `Private` unconditionally in both directions—a
    /// precondition on existing replicas would hand third parties a veto over
    /// the admin. Effects are asymmetric: privatizing does not recall data
    /// already replicated, publicizing cannot be undone. Full semantics:
    /// design doc, "Transitions" under Bucket Visibility & Access.
    #[pallet::weight(...)]
    pub fn set_bucket_visibility(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        visibility: Visibility,
    ) -> DispatchResult;

    /// Add or update a member's role (admin only).
    /// 
    /// Admins cannot demote other admins - they can only:
    /// - Add new members (any role)
    /// - Update non-admin members' roles
    /// - Demote themselves (remove own admin status)
    /// 
    /// This prevents a single compromised admin from seizing control.
    ///
    /// Adding a `Reader` is what makes membership the read access list for a
    /// private bucket. Visibility is set separately via `set_bucket_visibility`—
    /// adding a Reader does not by itself make a bucket private.
    #[pallet::weight(...)]
    pub fn set_member(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        member: T::AccountId,
        role: Role,
    ) -> DispatchResult;

    /// Remove member from bucket (admin only).
    /// 
    /// Admins cannot remove other admins - they can only:
    /// - Remove non-admin members
    /// - Remove themselves
    /// 
    /// This prevents a single compromised admin from seizing control.
    /// 
    /// Note: This is a very primitive handling of multiple admin accounts, in
    /// practice you should be very careful with adding such accounts and should
    /// lean towards using a single one controlled by a DAO (contract, chain,
    /// ..).
    #[pallet::weight(...)]
    pub fn remove_member(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        member: T::AccountId,
    ) -> DispatchResult;



    // ─────────────────────────────────────────────────────────────
    // Storage agreements (per bucket, per provider)
    // ─────────────────────────────────────────────────────────────

    /// Request a replica storage agreement (anyone can request).
    /// 
    /// Creates a replica provider agreement:
    /// - Does NOT count toward min_providers for checkpoints
    /// - Syncs data autonomously from primaries or other replicas
    /// - Cannot be early-terminated (runs to expiry)
    /// - Unlimited number of replicas per bucket
    ///
    /// No syncability check—a private bucket with zero replicas is accepted;
    /// an unfulfillable agreement is the funder's own risk. Rationale: design
    /// doc, "No on-chain gate on replica creation".
    /// 
    /// The requester becomes the agreement owner (can top up, transfer
    /// ownership).
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket to add a replica for
    /// - `provider`: The provider to request an agreement with
    /// - `max_bytes`: Maximum storage quota for this agreement
    /// - `duration`: How long the agreement should last
    /// - `max_payment`: Upper bound on storage payment. Actual payment is calculated
    ///   as `provider.price_per_byte * max_bytes * duration`. Fails if this exceeds
    ///   `max_payment` (protects against price changes between query and submission).
    /// - `replica_params`: Replica-specific parameters:
    ///   - `sync_balance`: Transferred from requester to fund per-sync payments at
    ///     the provider's `replica_sync_price`. When exhausted, replica stops
    ///     receiving sync payments but remains bound until expiry. Can top up via
    ///     `top_up_replica_sync_balance`.
    ///   - `min_sync_interval`: Minimum blocks between sync confirmations. Set based
    ///     on expected bucket activity. 0 for no time-based limit.
    #[pallet::weight(...)]
    pub fn request_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        max_bytes: u64,
        duration: BlockNumberFor<T>,
        max_payment: BalanceOf<T>,
        replica_params: ReplicaRequestParams<T>,
    ) -> DispatchResult;

    /// Accept a pending agreement request (provider only).
    /// 
    /// Creates the storage agreement and adds the provider to the bucket.
    /// For primary agreements: provider is added to `bucket.primary_providers`.
    /// For replica agreements: provider can start syncing immediately.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket with the pending request
    #[pallet::weight(...)]
    pub fn accept_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
    ) -> DispatchResult;

    /// Reject a pending agreement request (provider only).
    /// 
    /// Refunds the locked payment to the original requester.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket with the pending request to reject
    #[pallet::weight(...)]
    pub fn reject_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
    ) -> DispatchResult;

    /// Withdraw a pending agreement request before provider accepts.
    /// 
    /// Only the original requester can withdraw. Refunds the locked payment.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket with the pending request
    /// - `provider`: The provider the request was made to
    #[pallet::weight(...)]
    pub fn withdraw_agreement_request(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
    ) -> DispatchResult;

    /// Top up quota for an existing agreement (owner only).
    /// Increases max_bytes, does not change duration.
    /// Actual payment = provider.price_per_byte * additional_bytes * remaining_duration.
    /// Fails if calculated payment > max_payment.
    #[pallet::weight(...)]
    pub fn top_up_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        additional_bytes: u64,
        max_payment: BalanceOf<T>,
    ) -> DispatchResult;

    /// Extend agreement duration (immediate, no provider approval needed).
    /// 1. Settles current period: releases payment to provider for elapsed time
    /// 2. Calculates and locks new payment for extension at current provider prices
    /// 3. Updates end date to now + additional_duration
    /// 4. Updates agreement.price_per_byte (and sync_price for replicas) to current prices
    /// 
    /// **Price change rules:**
    /// - If provider's current price <= agreement's locked price: anyone can extend
    /// - If provider's current price > agreement's locked price: only owner can extend
    /// This enables permissionless persistence for frozen buckets while protecting
    /// owners from unwanted price increases.
    /// 
    /// Actual payment = provider.price_per_byte * current_max_bytes * additional_duration.
    /// For replicas: also requires topping up sync_balance proportionally.
    /// Fails if calculated payment > max_payment.
    /// 
    /// Also fails if:
    /// - Duration below provider's min_duration or above max_duration
    /// - Provider has globally paused extensions (settings.accepting_extensions == false)
    /// - Provider has blocked extensions for this specific bucket (agreement.extensions_blocked == true)
    #[pallet::weight(...)]
    pub fn extend_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        additional_duration: BlockNumberFor<T>,
        max_payment: BalanceOf<T>,
    ) -> DispatchResult;

    /// Transfer agreement ownership (current owner only).
    /// 
    /// The new owner can top up quota and transfer ownership further.
    /// Useful for selling agreement slots or transferring to a DAO.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket containing the agreement
    /// - `provider`: The provider of the agreement to transfer
    /// - `new_owner`: Account that will become the new agreement owner
    #[pallet::weight(...)]
    pub fn transfer_agreement_ownership(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        new_owner: T::AccountId,
    ) -> DispatchResult;

    /// End agreement with pay/burn decision.
    /// 
    /// **After expiry:** Owner can call within T::SettlementTimeout to settle.
    /// If owner doesn't act, provider can call claim_expired_agreement.
    /// 
    /// **Before expiry (early termination):** Only admin can call, only for primary
    /// providers. The full remaining payment is subject to the action (not pro-rated).
    /// 
    /// **Why early termination for primaries?**
    /// Admin needs ability to remove hostile or misbehaving primary providers.
    /// Without this, a malicious primary could hold the bucket hostage until expiry.
    /// Primary providers are admin-controlled for write coordination; admin must
    /// maintain control over who can accept writes.
    /// 
    /// **Replicas cannot be early-terminated:** There's no use case, and allowing
    /// it would violate the principle of least surprise. A business checking on a
    /// bucket sees "5 providers with agreements until May" and concludes all is
    /// well - they shouldn't find the bucket dead the next day because someone
    /// terminated agreements early. If unhappy with a provider, simply don't extend.
    /// 
    /// Note: For primary agreements, admin is the owner (created via request_primary_agreement).
    /// Admin has no special privileges over replica agreements.
    #[pallet::weight(...)]
    pub fn end_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        action: EndAction,
    ) -> DispatchResult;

    /// Claim payment for expired agreement (provider only).
    /// Can only be called after agreement expired + T::SettlementTimeout.
    /// Client forfeited their right to burn by not acting in time.
    #[pallet::weight(...)]
    pub fn claim_expired_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
    ) -> DispatchResult;

    /// Request a primary storage agreement (admin only).
    /// 
    /// Creates a primary (admin-added) provider agreement:
    /// - Counts toward min_providers for checkpoints
    /// - Stored in bucket.primary_providers (limited to T::MaxPrimaryProviders)
    /// - Can be early-terminated by admin
    /// 
    /// Fails if bucket has reached T::MaxPrimaryProviders limit.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket to add a primary provider for
    /// - `provider`: The provider to request an agreement with
    /// - `max_bytes`: Maximum storage quota for this agreement
    /// - `duration`: How long the agreement should last
    /// - `max_payment`: Upper bound on storage payment. Actual payment is calculated
    ///   as `provider.price_per_byte * max_bytes * duration`. Fails if this exceeds
    ///   `max_payment` (protects against price changes between query and submission).
    #[pallet::weight(...)]
    pub fn request_primary_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        max_bytes: u64,
        duration: BlockNumberFor<T>,
        max_payment: BalanceOf<T>,
    ) -> DispatchResult;

    /// Remove a slashed provider from a bucket (anyone can call).
    /// 
    /// After a provider is slashed (failed a challenge), they should be removed
    /// from the bucket's provider lists. This is permissionless because:
    /// - Slashing is already a clear on-chain signal of failure
    /// - Keeping slashed providers in lists is misleading
    /// - No payment/burn decision needed (the slash already handled consequences)
    /// 
    /// Removes the agreement entirely. For primary providers, also removes from
    /// bucket.primary_providers and adjusts the snapshot bitfield if they were in it.
    /// 
    /// The agreement's remaining payment is handled as follows:
    /// - If slashed while agreement was active: remaining payment returned to owner
    ///   (provider already punished via stake slash, client shouldn't also lose payment)
    #[pallet::weight(...)]
    pub fn remove_slashed(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
    ) -> DispatchResult;

    // ─────────────────────────────────────────────────────────────
    // Checkpoints
    // ─────────────────────────────────────────────────────────────

    /// Submit a new checkpoint with provider signatures (writers/admin only).
    /// 
    /// Creates a new canonical state (new `Commitment`).
    /// Requires at least min_providers signatures from providers in bucket.primary_providers.
    /// For frozen buckets: start_seq must equal frozen_start_seq (only leaf_count can increase).
    pub fn checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        commitment: Commitment,
        nonce: u64,
        signatures: BoundedVec<(T::AccountId, Signature), T::MaxPrimaryProviders>,
    ) -> DispatchResult;

    /// Extend an existing checkpoint's provider bitfield (anyone can call).
    /// 
    /// Adds additional provider signatures to the current snapshot without changing
    /// the mmr_root, start_seq, or leaf_count. This is permissionless because:
    /// - It only adds accountability (more providers are now challengeable)
    /// - It cannot change the canonical state
    /// - Signatures are verified on-chain
    /// 
    /// Providers added this way become liable for the snapshot state.
    pub fn extend_checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        additional_signatures: BoundedVec<(T::AccountId, Signature), T::MaxPrimaryProviders>,
    ) -> DispatchResult;

    // ─────────────────────────────────────────────────────────────
    // Challenges
    // ─────────────────────────────────────────────────────────────
    //
    // Three challenge modes exist for different scenarios:
    //
    // **challenge_checkpoint** - Best for cold/stable buckets:
    // - Infrequent writes mean snapshot stays stable
    // - No race conditions between challenge and new checkpoints
    // - Guarantees min_providers are always challengeable via on-chain state
    // - No need for challenger to store signatures locally
    //
    // **challenge_offchain** - Best for hot/active buckets:
    // - Frequent writes cause snapshot races (new checkpoint may not include
    //   the provider you want to challenge)
    // - Writers have fresh signatures from their commits
    // - Writers are the natural challengers (they're active participants)
    // - Signatures are recoverable from block history if needed
    //
    // **challenge_replica** - For replica providers:
    // - Uses the replica's on-chain sync confirmation (last_synced_root)
    // - No signature needed - chain already has their commitment
    // - Replicas are liable for roots they've confirmed synced to
    //
    // For hot buckets, challenge_checkpoint may fail due to race conditions,
    // but this is acceptable: active writers have signatures and can use
    // challenge_offchain. The snapshot primarily protects cold/archival data
    // where nobody has recent signatures or doesn't bother to dig them up.
    //
    // **Who may challenge, and at what cost (all three modes):**
    // Any signed account may challenge, with one restriction: on a `Private`
    // bucket, challenging a provider whose agreement role is `Primary`
    // requires being a bucket member or the owner of a primary agreement on
    // the bucket (`NotAuthorizedForPrivateBucket`; replica-agreement owners
    // deliberately excluded—rationale in the design doc, "The Challenge
    // Game"). The gate reads the challenged provider's role from its *current*
    // agreement—the same lookup that yields `AgreementNotFound`—so an ended
    // agreement means no challenge, never a stale role.
    // The challenger's deposit must cover the
    // provider's on-chain response cost (generously over-estimated; excess is
    // refunded on resolution). On a valid response the provider's stake is
    // never touched—only its response transaction fee is at issue, and the
    // deposit reimburses it. How much of that cost the provider is made to bear
    // depends on the challenger:
    //
    //   - **Authorized accounts** — `is_authorized(who, bucket)` is true:
    //     bucket members (Admin/Writer/Reader) or the owner of any storage
    //     agreement on the bucket (so replica funders qualify). The provider is
    //     made to bear a fraction of the cost per the cost-split table
    //     (response-time based); the challenger's deposit covers the rest. The
    //     challenger's share never drops below 50%, so the split is leverage to
    //     pressure the provider into serving—not a cheap recovery channel, even
    //     for the owner.
    //
    //   - **General public** — everyone else: the challenger pays 100%; the
    //     provider is reimbursed in full and loses no money on a valid response.
    //     Still able to detect and slash a dead provider, and to recover a
    //     chunk—at full cost. No split for two reasons: (1) a provider can't
    //     serve everyone equally well, so a stranger being made to wait isn't
    //     evidence of fault; (2) anti-DDoS—if strangers got the split, a crowd
    //     could each pay little while collectively draining the provider.
    //
    // The tier is evaluated once at challenge creation and snapshotted in the
    // `Challenge` (see `Challenge.authorized`); membership or agreement changes
    // afterwards do not affect an open challenge.
    //
    // `is_authorized` is the single authorization predicate shared with
    // private-bucket read access control. No per-challenge rate limiting or
    // stored "last challenge" timestamp is needed: full-cost public challenges
    // are self-limiting (the challenger pays in full every time) and leave an
    // honest provider financially unharmed.

    /// Challenge on-chain checkpoint (no signatures needed).
    /// Provider must be in current snapshot's provider list.
    /// On a `Private` bucket the caller must be a member or primary-agreement
    /// owner (`NotAuthorizedForPrivateBucket`); snapshot providers are
    /// primaries by construction, so the gate always applies here.
    /// 
    /// NOTE: May race with new checkpoints in hot buckets. If the provider is
    /// no longer in the snapshot when the transaction executes, this fails.
    /// For hot buckets, prefer challenge_offchain with the signature you have.
    pub fn challenge_checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        target: ChunkLocation,
    ) -> DispatchResult;

    /// Challenge off-chain commitment (requires provider signature).
    /// Works regardless of current snapshot state - the signature proves
    /// the provider committed to this data.
    /// On a `Private` bucket, the gate applies iff the challenged provider's
    /// current agreement has role `Primary`
    /// (`NotAuthorizedForPrivateBucket`; role-based gate, see above).
    /// 
    /// Preferred for hot buckets where snapshots change frequently.
    pub fn challenge_offchain(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        commitment: Commitment,
        target: ChunkLocation,
        nonce: u64,
        provider_signature: Signature,
    ) -> DispatchResult;

    /// Challenge a replica based on their on-chain sync confirmation.
    /// Uses the replica's last_synced_root stored in their agreement.
    /// No signature needed - the chain already has their commitment.
    /// Open to everyone regardless of bucket visibility (role-based gate).
    pub fn challenge_replica(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        target: ChunkLocation,
    ) -> DispatchResult;

    // ─────────────────────────────────────────────────────────────
    // Replica sync
    // ─────────────────────────────────────────────────────────────

    /// Replica confirms sync to one or more MMR roots.
    /// 
    /// **Why this exists:**
    /// Replicas sync autonomously and need to prove they actually have the data.
    /// By signing which roots they've synced to, replicas become challengeable for
    /// that data. The chain validates against current snapshot and historical_roots
    /// to ensure the replica isn't claiming a fabricated root.
    /// 
    /// **Why historical roots (prime-bucketed)?**
    /// Replicas may lag behind the current snapshot. Rather than requiring exact
    /// sync to current state (which races with new checkpoints), we accept sync
    /// confirmations against recent historical roots. Prime-based bucketing (see
    /// `Bucket.historical_roots`) provides O(1) storage with logarithmic time
    /// coverage, allowing replicas to confirm sync even when slightly behind.
    /// 
    /// **Matching logic:**
    /// The chain checks positions in order: current snapshot first, then historical
    /// positions 0-5. The first position where the replica's claimed root matches
    /// the on-chain root is used. This means replicas are credited for the most
    /// recent state they've synced to, even if they also have older roots.
    /// 
    /// **Rate limiting:**
    /// Two checks prevent excessive sync confirmations:
    /// 1. The matched root must differ from `last_sync.0` (must be new state)
    /// 2. `current_block >= last_sync.1 + min_sync_interval` (per-agreement)
    /// 
    /// The first check ensures payment only for actual sync work. The second
    /// prevents hot buckets (writes every block) from causing excessive on-chain
    /// sync confirmations. `min_sync_interval` is set per-agreement at creation,
    /// based on expected bucket activity. Set to 0 for no time-based limit.
    /// 
    /// Replicas are already paid for storage via `payment_locked` (like primaries),
    /// which covers storage costs (slashing risk is negligible if they do their
    /// job properly). The `sync_price` separately
    /// compensates for sync work: bandwidth costs, incentivizing other providers
    /// to serve data (they may refuse or deprioritize), verification compute, and
    /// tx costs. Sync-specific risks (e.g., uncooperative providers causing sync
    /// failures) should be negligible if the replica syncs regularly.
    /// 
    /// On success (both checks pass):
    /// - Updates replica's `last_sync` to `(matched_root, current_block)`
    /// - Pays sync_price from replica's sync_balance
    /// - Emits ReplicaSynced event with position_matched for performance tracking
    ///   (position 0 = current snapshot, 1-6 = historical positions, higher = more lag)
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket the replica is syncing
    /// - `roots`: Array of optional MMR roots [current, pos0, pos1, pos2, pos3, pos4, pos5].
    ///   Replica sets Some(root) for positions they have, None for positions they don't.
    /// - `signature`: Provider signature over the roots array
    #[pallet::weight(...)]
    pub fn confirm_replica_sync(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        /// Array of optional MMR roots: [current, pos0, pos1, pos2, pos3, pos4, pos5]
        /// Provider signs this to attest which roots they have.
        roots: [Option<H256>; 7],
        signature: Signature,
    ) -> DispatchResult;

    /// Top up a replica's sync balance (agreement owner or anyone).
    /// 
    /// Adds funds to the replica's sync_balance for future sync payments.
    /// This is permissionless because it only benefits the replica (more funds
    /// to pay for syncs) and the bucket (more redundancy).
    #[pallet::weight(...)]
    pub fn top_up_replica_sync_balance(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        amount: BalanceOf<T>,
    ) -> DispatchResult;

    /// Provider responds to challenge with proof.
    /// 
    /// Must provide the challenged chunk with Merkle proofs, or prove the data
    /// was legitimately deleted (newer commitment with higher start_seq), or
    /// show the challenged state has been superseded by canonical.
    /// 
    /// Parameters:
    /// - `challenge_id`: The challenge to respond to (deadline + index)
    /// - `response`: Proof, Deleted, or Superseded response
    #[pallet::weight(...)]
    pub fn respond_to_challenge(
        origin: OriginFor<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        response: ChallengeResponse<T>,
    ) -> DispatchResult;
}

pub enum EndAction {
    /// Pay provider in full
    Pay,
    /// Burn portion, pay rest (0-100%)
    Burn { burn_percent: u8 },
}

pub enum RemovalReason {
    /// Provider was slashed for failing a challenge
    Slashed,
    /// Admin terminated agreement early
    AdminTerminated,
    /// Agreement expired naturally
    Expired,
}

pub enum ChallengeResponse<T: Config> {
    /// Provide the chunk with proofs
    Proof {
        chunk_data: BoundedVec<u8, T::MaxChunkSize>,
        mmr_proof: MmrProof,
        chunk_proof: MerkleProof,
    },
    /// Data was deleted - show newer commitment without this seq.
    /// Admin signature proves the admin authorized the deletion (new MMR excludes the challenged data).
    /// Only admins can delete data (by increasing start_seq), so the signature must be from an admin.
    /// Provider signature not needed - they're submitting this response.
    Deleted {
        new_mmr_root: H256,
        new_start_seq: u64,
        /// Block at which the admin signed the deletion commitment. Used as
        /// the `nonce` in `CommitmentPayload` and recency-checked by the
        /// pallet to prevent signature replay.
        nonce: u64,
        admin: T::AccountId,
        admin_signature: Signature,
    },
    /// Challenged state has been superseded by a larger canonical checkpoint.
    /// Valid when: canonical.start_seq <= challenged_seq < canonical.start_seq + canonical.leaf_count
    /// (The leaf exists in canonical - challenger should challenge the snapshot instead)
    /// (For challenged_seq < canonical.start_seq, use Deleted response instead)
    /// (For challenged_seq >= canonical_end, provider is liable - must use Proof)
    Superseded,
}
```

---

## Off-Chain: Provider Node API

The provider node exposes a JSON-over-HTTP API (axum) on, by default,
`http://localhost:3333`. Endpoints fall into three groups:

1. **Health & info** — public, unauthenticated.
2. **Layer-0 blob storage** — content-addressed node upload, existence check,
   commit, read, proofs, deletion. Mutating endpoints require auth.
3. **Replica sync** — peaks, subtree, bulk node fetch, sync status. Used by
   replica providers; read-only.

### Authentication & RBAC

Mutating Layer-0 endpoints (`PUT /node`, `POST /commit`, `POST /delete`) and
authenticated read endpoints require an `Authorization` header. The provider node verifies an sr25519 signature
locally and resolves the caller's role via a TTL-cached query against the
chain's `Buckets` storage (`bucket.members`).

> **⚠️ Under-specified — [#304](https://github.com/paritytech/web3-storage/issues/304).**
> This scheme grew organically across several crates and needs one source of
> truth: the wire format is currently defined twice (Rust `provider-negotiation`
> + TS `core`) and hand-synced; the provider also accepts a `<Bytes>`-wrapped
> form (what wallets / PAPI `signBytes` send) not documented below; and the
> signed message binds only method + bucket + timestamp — **no body or provider
> binding**, leaving a replay window (default 5 min skew). #304 tracks the
> canonical definition + the binding/replay fix.

```
Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<unix_timestamp>

Signed message: "web3storage:<METHOD>:<bucket_id>:<unix_timestamp>"
```

Rules:
- `<unix_timestamp>` must be within `--auth-max-skew` (default 5 minutes) of
  the provider's clock — otherwise `401 TimestampExpired`.
- Required role per endpoint: `Reader` for reads of access-controlled data,
  `Writer` for uploads/commits, `Admin` for delete and other destructive ops.
- The membership cache uses stale-while-revalidate: if the chain is briefly
  unreachable, cached membership keeps working.

### Content-Addressed Storage

Everything is content-addressed by hash. Upload is bottom-up: children must exist before parent.

```
Upload Node (chunk or internal node)
────────────────────────────────────
PUT /node
Authorization: Web3Storage <...>       # Writer or Admin

Request:
{
  "bucket_id": 1234,                   // u64
  "hash": "0xabc...",
  "data": "<base64 encoded>",
  "children": ["0xchild1...", "0xchild2..."] | null  // null for leaf chunks
}

Note: HTTP API is used for simplicity and firewall-friendliness. Binary protocols
(e.g., libp2p streams) could be added later for efficiency. Base64 encoding adds
~33% overhead but keeps the API JSON-friendly. For high-throughput scenarios,
consider a binary endpoint or chunked transfer encoding.

Response (200 OK):
{ "stored": true }

Response (400 Bad Request):
{ "error": "children_missing", "missing": ["0xchild2..."] }

Response (507 Insufficient Storage):
{ "error": "quota_exceeded", "used": 1000000, "max": 1000000 }
```

### Sync Protocol

Client discovers which nodes are missing before uploading.

```
Check Existence (batched)
─────────────────────────
POST /exists

Request:
{
  "bucket_id": "0x1234...",
  "hashes": ["0xabc...", "0xdef...", "0x123...", ...]
}

Response:
{
  "exists": ["0xabc...", "0x123..."],
  "missing": ["0xdef..."]
}

Note: Client traverses tree top-down, checking level by level.
If a node exists, skip its subtree. Upload missing nodes bottom-up.
```

### Commit

After uploading, client requests provider to add data_root(s) to MMR.

```
Commit
──────
POST /commit

Request:
{
  "bucket_id": "0x1234...",
  "data_roots": ["0xroot1...", "0xroot2..."],  // roots to add to MMR
  "nonce": 12345  // CommitmentPayload nonce (block at expected submission)
}

Response (200 OK):
{
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 7,  // number of leaves after the commit
  "leaf_indices": [5, 6],  // indices assigned to each data_root
  "provider_signature": "0x...",
  "nonce": 12345  // echo of the nonce the provider signed over
}

Response (400 Bad Request):
{ "error": "root_not_found", "missing": ["0xroot2..."] }
```

### Read

```
Read Chunks
───────────
GET /read?data_root=0x...&offset=0&length=2097152

Response:
{
  "chunks": [
    { "hash": "0xabc...", "data": "<base64>", "proof": [...] },
    ...
  ]
}
```

### Other Endpoints

```
Provider Info
─────────────
GET /info

Response:
{
  "status": "healthy",
  "version": "0.1.0"
}

Note: Provider settings (prices, durations, accepting flags) are intentionally
omitted — the chain is the source of truth. Clients should query the chain via
runtime API for authoritative provider information.

Download Node
─────────────
GET /node?hash=0x...

Response (200 OK):
{
  "hash": "0xabc...",
  "data": "<base64 encoded>",
  "children": ["0xchild1...", "0xchild2..."] | null
}

Response (404 Not Found):
{ "error": "not_found" }

Get Commitment (for challenge_offchain)
───────────────────────────────────────
GET /commitment?bucket_id=1234&nonce=12345

Response:
{
  "bucket_id": 1234,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "provider_signature": "0x...",
  "nonce": 12345
}

Note: The returned signature covers a `CommitmentPayload` with the real
`leaf_count`; `challenge_offchain` reconstructs the payload from the
`commitment` the challenger passes, so the same values returned here must be
passed on-chain unchanged.

Get Checkpoint Signature (for checkpoint extrinsic)
───────────────────────────────────────────────────
GET /checkpoint-signature?bucket_id=1234&nonce=12345

Response:
{
  "bucket_id": 1234,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "provider_signature": "0x...",
  "nonce": 12345
}

Note: Signs the same payload as `/commitment`; kept as a separate endpoint
for the checkpoint workflow, where the signature goes into the
`checkpoint`/`extend_checkpoint` signatures BoundedVec.

Get MMR Proof
─────────────
GET /mmr_proof?bucket_id=0x...&leaf_index=5

Response:
{
  "leaf": { "data_root": "0x...", "data_size": 2097152, "total_size": 52428800 },
  "proof": { "peaks": [...], "siblings": [...] }
}

Get Chunk Proof
───────────────
GET /chunk_proof?data_root=0x...&chunk_index=3

Response:
{
  "chunk_hash": "0xabc...",
  "proof": { "siblings": [...], "path": [...] }
}

Response (404 Not Found):
{ "error": "data_root_not_found" }

Delete Data (admin only)
────────────────────────
POST /delete
Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>
  // admin-signed header (same scheme as other mutating endpoints);
  // the signer must be an Admin member of the bucket

Request:
{
  "bucket_id": "0x1234...",
  "new_start_seq": 10,
  "nonce": 12345  // CommitmentPayload nonce for the post-deletion signature
}

Response (200 OK):
{
  "mmr_root": "0xnew...",
  "start_seq": 10,
  "leaf_count": 5,
  "provider_signature": "0x...",
  "nonce": 12345
}

Response (400 Bad Request):
{ "error": "invalid_signature" }

Response (403 Forbidden):
{ "error": "not_admin" }

Note: Only bucket admins can delete data. This triggers deletion of data before
new_start_seq. Provider returns new commitment covering remaining data. Admin
signature authorizes the deletion and serves as proof if challenged later.

List Buckets
────────────
GET /buckets

Response:
{
  "buckets": [
    { "bucket_id": "0x1234...", "mmr_root": "0x...", "start_seq": 0, "leaf_count": 42 },
    { "bucket_id": "0x5678...", "mmr_root": "0x...", "start_seq": 5, "leaf_count": 10 }
  ]
}

Health Check
────────────
GET /health

Response (200 OK):
{ "status": "healthy", "version": "0.1.0" }

Stats
─────
GET /stats

Response:
{
  "provider_id": "5G...",            // SS58 address
  "total_buckets": 3,
  "total_nodes": 1234,
  "total_bytes": 42949672960,
  "buckets": [
    { "bucket_id": 1234, "nodes": 500, "bytes": 21474836480, ... },
    ...
  ]
}

Note: Public observability endpoint. Useful for operators and the
Prometheus/Grafana setup in `docs/`.
```

### Replica Sync Status

```
Get Historical Roots (informational)
────────────────────────────────────
GET /replica/historical_roots?bucket_id=1234

Response:
{
  "bucket_id": 1234,
  "current_root": "0xfed...",
  "historical_roots": ["", "", "", "", "", ""],
  "snapshot_block": 0
}

Note: Provider nodes do NOT track historical roots — only the chain does, in
`Bucket.historical_roots`. This endpoint returns the local current MMR root
and placeholder entries for the historical positions; clients building
`confirm_replica_sync` calls should query the chain via runtime API.

Get Replica Sync Status
───────────────────────
GET /replica/sync_status?bucket_id=1234

Response:
{
  "bucket_id": 1234,
  "local_mmr_root": "0xfed...",
  "local_leaf_count": 42,
  "last_sync_block": null,
  "syncing": false
}
```

### Replica Sync API

Replicas sync data autonomously from primaries or other replicas using a
top-down Merkle traversal. This section describes the sync protocol.

**Sync flow overview:**

1. Replica queries the **chain** for current bucket state (MMR root from checkpoint)
2. Replica fetches MMR structure (peaks) from any provider, verifying against chain root
3. Replica performs top-down traversal, checking which nodes it already has
4. Replica fetches missing nodes from providers, verifying hashes along the way
5. Once fully synced, replica confirms on-chain to receive per-sync payment

**Why chain-first?**

The chain checkpoint is the source of truth. Fetching the root from a provider
would require trusting that provider. By getting the root from the chain first,
the replica can verify all fetched data against a trusted commitment.

```
Get MMR Peaks (given trusted root from chain)
─────────────────────────────────────────────
GET /mmr_peaks?bucket_id=0x...

Response:
{
  "bucket_id": "0x1234...",
  "mmr_root": "0xfed...",
  "peaks": ["0xpeak1...", "0xpeak2...", ...]
}

Note: Replica already knows the trusted mmr_root from the chain. It fetches
peaks from a provider and verifies: hash(peaks) == trusted_root. If verification
fails, try another provider. Once verified, use peaks to start top-down traversal.

Get MMR Subtree
───────────────
GET /mmr_subtree?bucket_id=0x...&peak_index=0&depth=2

Request: Fetch nodes in an MMR subtree starting from a peak.
- peak_index: which peak to start from (0 = leftmost)
- depth: how many levels to fetch (0 = just the peak, 1 = peak + children, etc.)

Response:
{
  "nodes": [
    { "position": 0, "hash": "0xabc...", "children": [1, 2] },
    { "position": 1, "hash": "0xdef...", "children": [3, 4] },
    { "position": 2, "hash": "0x123...", "children": [5, 6] },
    ...
  ]
}

Note: Replica can batch requests by depth level. Check which hashes match
locally stored nodes, then fetch children of missing nodes.

Note: To check which nodes exist on a provider, use the existing POST /exists
endpoint from the Sync Protocol section above.

Fetch Nodes (batched, for sync)
───────────────────────────────
POST /fetch_nodes

Request:
{
  "bucket_id": "0x1234...",
  "hashes": ["0xdef...", "0x456...", ...]
}

Response:
{
  "nodes": [
    { "hash": "0xdef...", "data": "<base64>", "children": ["0xchild1...", "0xchild2..."] },
    { "hash": "0x456...", "data": "<base64>", "children": null }  // leaf chunk
  ]
}

Note: Bulk fetch of nodes by hash. More efficient than individual GET /node
requests when syncing many nodes.
```

**Top-down sync algorithm:**

```
1. Query chain for bucket's current snapshot (mmr_root, start_seq, leaf_count)
   Also note historical_roots for fallback positions
2. Fetch mmr_peaks from any provider
3. Verify: hash(peaks) == trusted mmr_root from chain
   If mismatch, try another provider
4. Compare verified peaks with locally stored peaks
5. For each differing peak:
   a. Fetch subtree level by level (breadth-first)
   b. At each level, check which nodes exist locally
   c. Fetch missing nodes from any available provider
   d. Verify fetched nodes: hash(data) == expected_hash
   e. Continue to children of newly fetched nodes
6. Once all nodes fetched and verified:
   a. Build signature over roots array matching on-chain historical_roots
   b. Submit confirm_replica_sync on-chain
   c. Receive per-sync payment from sync_balance
```

**Why top-down?**

- Enables early termination: if a node hash matches, skip entire subtree
- Natural deduplication: unchanged subtrees are detected at first node
- Verifiable: each node's hash is verified before fetching children
- Resumable: sync state is just "which nodes are missing"

**Historical roots for sync confirmation:**

When confirming sync on-chain, replicas provide roots for multiple positions:
- Position 0: current snapshot root
- Positions 1-6: historical roots at prime intervals (3, 7, 11, 23, 47, 113 blocks)

This gives replicas a ~1 minute window to sync without racing against new
checkpoints. If a new checkpoint arrives while syncing, the replica can still
confirm using an older historical root they successfully synced to.

---

## Data Structures

### Commitment & ChunkLocation

`Commitment` groups the `(mmr_root, start_seq, leaf_count)` triplet that
identifies an MMR commitment over a contiguous range of leaves. It is a field
group inside `CommitmentPayload` and `BucketSnapshot`,
and the single argument the checkpoint/challenge extrinsics take in place of
three loose fields.

```rust
pub struct Commitment {
    /// Root of MMR containing all data_roots
    pub mmr_root: H256,
    /// Sequence number of the first leaf covered by this commitment
    pub start_seq: u64,
    /// Number of leaves covered by this commitment
    pub leaf_count: u64,
}

// Canonical range: [start_seq, start_seq + leaf_count)
```

`ChunkLocation` is the companion *position* type (`Commitment` is a *range*):
the exact chunk a challenge targets.

```rust
pub struct ChunkLocation {
    /// Index of the challenged leaf within the MMR
    pub leaf_index: u64,
    /// Index of the challenged chunk within the leaf's data
    pub chunk_index: u64,
}
```

### Signed Commitment

Both payloads live in `storage_primitives` so the pallet, provider node, and
client SDK encode/decode identically. They each carry a `version: u8` for
forward compatibility.

```rust
pub struct CommitmentPayload {
    /// Protocol version for future compatibility (CURRENT_VERSION = 2)
    pub version: u8,
    /// Reference to on-chain bucket. Mandatory — there is no anonymous /
    /// "best-effort" commitment mode in the current implementation.
    pub bucket_id: BucketId,
    /// MMR commitment being signed over
    pub commitment: Commitment,
    /// Replay-protection nonce — the anchor (relay-chain) block number at the
    /// time the signer signed. Checked against `current_anchor_block()`.
    pub nonce: u64,
}
```

### MMR Leaf

```rust
pub struct MmrLeaf {
    /// Merkle root of chunk tree
    pub data_root: H256,
    /// Size of content under this data_root
    pub data_size: u64,
    /// Cumulative unique bytes in MMR at this point
    pub total_size: u64,
}
// Sequence number is implicit: start_seq + leaf_position
```

### Merkle Proofs

```rust
pub struct MerkleProof {
    /// Sibling hashes from leaf to root
    pub siblings: Vec<H256>,
    /// Path bits (0 = left, 1 = right)
    pub path: Vec<bool>,
}

pub struct MmrProof {
    /// Peaks of the MMR
    pub peaks: Vec<H256>,
    /// Proof from leaf to peak
    pub leaf_proof: MerkleProof,
}
```

---

## Challenge Protocol

### Timeline

```
1. Challenger initiates challenge on-chain
   └─ Provides: signed commitment, leaf_index, chunk_index
   └─ Locks a generously over-estimated deposit covering the provider's
      on-chain response cost (margin for fee fluctuations)
   └─ Tier determined by is_authorized(challenger, bucket):
      authorized (member or agreement owner) vs. general public

2. Challenge window opens (1-2 days)
   └─ Provider must respond within window
   └─ Provider pays its response tx fee from its own account—NOT its stake

3a. Provider responds with valid proof
    └─ Challenge rejected; stake untouched
    └─ Provider's response fee is reimbursed from the challenger's deposit:
       • General public  → 100% reimbursed; provider bears nothing (money)
       • Authorized      → reimbursed per the cost-split table; provider
         is made to bear the remaining fraction (response-time based:
         fast → provider bears less; slow → more). The challenger's
         share never drops below 50%.
    └─ Any deposit beyond what was used is returned to the challenger
    └─ Challenger obtains the chunk via the on-chain proof (full on-chain
       cost applies—a last-resort recovery path, not a cheap bulk channel)

3b. Provider responds with deletion proof
    └─ Shows newer admin-signed commitment with start_seq > challenged seq
    └─ Challenge rejected (data was legitimately deleted)
    └─ Treated as a valid response: provider's fee reimbursed as in 3a,
       remainder returned to challenger; stake untouched

3c. Provider fails to respond / invalid proof
    └─ Provider's contract stake fully slashed
    └─ Challenger made whole from the slash: deposit refunded, tx fees
       reimbursed—but no reward beyond actual costs (no profit motive
       for forcing slashes), regardless of tier
    └─ Clear on-chain evidence of provider fault
```

**Why this cost model?**
- **Strangers can't drain a provider (anti-DDoS)**: A public challenge leaves an honest provider whole in money terms (fee fully reimbursed, stake untouched). If strangers got the split instead, a crowd could each pay little while collectively draining the provider; full-cost-per-stranger makes the attackers' cost scale with the damage. A stranger can still impose on-chain work and a reputation hit, but cannot extract value or grind down stake.
- **A provider can't serve everyone equally**: so a stranger being made to wait (e.g. under a lot of load) isn't evidence of fault—unlike a paying counterparty's unanswered request.
- **Owners get leverage, not cheap recovery**: the split lets a counterparty pressure the provider into serving, but with the challenger's share floored at 50% of a high on-chain cost, it stays a last-resort tool—recovering data at scale this way is unreasonably expensive even for the owner.
- **Monetary exposure is bounded to chosen counterparties**: a provider is made to bear cost only for accounts it accepted agreements with (or the admin added)—it controls that risk by vetting whom it signs with.
- **Off-chain resolution preferred**: answering on-chain means posting the data as a transaction—far costlier than serving the same bytes off-chain (the bandwidth is spent either way)—plus in-window hassle and reputation damage, even when the fee is reimbursed. So the provider serves directly.

> **Note on the deposit/fee mechanic.** The deposit is sized to the *transaction cost* of the provider's response, not a slice of stake. A simple implementation: the provider pays the response fee from its account when it submits the proof, and the challenge-resolution logic refunds that fee out of the locked deposit (in full for public challengers, or the table fraction for authorized ones), returning any remainder to the challenger. No stake movement occurs on a valid response—stake is only ever touched by the slash in 3c.

### Verification

```rust
fn verify_challenge_response(
    challenge: &Challenge,
    response: &ChallengeResponse,
    bucket: &Bucket,
) -> Result<(), Error> {
    match response {
        ChallengeResponse::Proof { chunk_data, mmr_proof, chunk_proof } => {
            // 1. Verify chunk hash
            let chunk_hash = blake2_256(chunk_data);
            
            // 2. Verify chunk is in data_root
            verify_merkle_proof(chunk_hash, challenge.chunk_index, chunk_proof, &mmr_proof.leaf.data_root)?;
            
            // 3. Verify data_root is in MMR
            verify_mmr_proof(&mmr_proof, challenge.leaf_index, &challenge.mmr_root)?;
            
            Ok(())
        }
        
        ChallengeResponse::Deleted { new_start_seq, admin, admin_signature, .. } => {
            // Note: We don't check frozen_start_seq here. Freeze protects canonical
            // checkpoints (enforced at checkpoint time), but off-chain deletions can
            // race with freeze. If admin signed a deletion, provider has valid defense
            // regardless of freeze state. Off-chain is "messy but functional."
            
            // Challenged seq must be before new start
            let challenged_seq = challenge.start_seq + challenge.leaf_index;
            ensure!(challenged_seq < *new_start_seq, Error::InvalidDeletionProof);
            
            // Verify admin signature on new commitment and that signer is bucket admin
            // ...
            
            Ok(())
        }
        
        ChallengeResponse::Superseded => {
            // Provider can defend if challenged state has been superseded by canonical.
            //
            // This defense covers three cases:
            // 1. Same data: challenged leaf exists in canonical with same content
            // 2. Forked data: challenged leaf was on a conflicting branch that lost
            // 3. Deleted data: canonical has moved past via deletion (start_seq increased)
            //
            // In all cases, canonical has "moved past" the challenged state. The provider
            // signed something that is no longer relevant - canonical supersedes it.
            //
            // Note: We don't require admin signature here (unlike Deleted defense).
            // Superseded is for when canonical evolved independently - possibly by a
            // different admin/provider. The provider shouldn't be slashed for state
            // that was superseded by canonical they weren't involved in.
            //
            // Deleted vs Superseded:
            // - Deleted: requires admin signature, works without canonical snapshot
            // - Superseded: requires canonical snapshot, works without admin signature
            // For challenged_seq < snapshot.start_seq, BOTH defenses are valid.
            // Provider can use whichever they have evidence for.
            //
            // Provider IS liable when challenged_seq >= canonical_end: they signed
            // something that extends BEYOND canonical, so they must Proof it.
            
            let snapshot = bucket.snapshot.as_ref().ok_or(Error::NoSnapshot)?;
            let challenged_seq = challenge.start_seq + challenge.leaf_index;
            let canonical_end = snapshot.start_seq + snapshot.leaf_count;
            
            // Superseded is valid if canonical has moved past challenged state:
            // - challenged_seq < snapshot.start_seq: canonical deleted past this
            // - challenged_seq < canonical_end: within canonical range
            // NOT valid if challenged_seq >= canonical_end: provider is liable
            ensure!(challenged_seq < canonical_end, Error::LeafBeyondCanonical);
            
            Ok(())
        }
    }
}
```

---

## Open Questions
