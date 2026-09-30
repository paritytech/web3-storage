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

Bucket creation and provider assignment are separate on-chain operations.
Four calls cover them:

| Call | Who | Effect |
| --- | --- | --- |
| `create_bucket` | anyone | Empty bucket, caller is sole admin |
| `create_bucket_with_primary` | the quoted account (`terms.owner`) | `create_bucket` + `add_primary_provider` in one atomic call |
| `add_primary_provider` | bucket admin | Primary agreement on an existing bucket |
| `add_replica_provider` | the quoted account (`terms.owner`) | Replica agreement on an existing bucket |

Every agreement is established by redeeming provider-signed terms: the
provider quotes `AgreementTerms` off-chain (`POST /negotiate`), signs them,
and the client submits the signed quote on-chain in a single call — there is
no on-chain request/accept round-trip. `terms.bucket` names which bucket the
quote is for: `BucketTarget::New` for `create_bucket_with_primary`, where the
bucket does not exist yet, and `BucketTarget::Existing(id)` for
`add_primary_provider` and `add_replica_provider`, which must target that
same bucket. The provider signs the target, so
it can decline one bucket without declining the account.

**Creating a bucket:**
1. Admin calls `create_bucket` → bucket created with the caller as sole
   admin, no providers, no data. The `bucket_id` is assigned here and never
   changes.
2. Or: admin requests primary `AgreementTerms` with
   `bucket: BucketTarget::New`, the provider signs them, admin calls
   `create_bucket_with_primary` →
   bucket created and the provider added as its first primary in the same
   block. One transaction instead of two; a caller that needs bucket and
   provider in the same block uses this path.

**Adding a primary provider:**
1. Admin requests primary `AgreementTerms` for the `bucket_id` from the
   provider off-chain; the provider signs them
2. Admin calls `add_primary_provider` with the signed terms →
   `StorageAgreement` created, provider appended to
   `bucket.primary_providers`, `ProviderAddedToBucket` emitted
3. Client uploads data to the provider. For a bucket that already has data,
   the client uploads the existing data too — primaries do not sync with
   each other (see Multi-Provider Coordination)
4. Client requests commit, provider signs → client has provider signature
5. Client calls `checkpoint` with provider signature → provider added to `snapshot.primary_signers` bitfield

Adding a second primary before the first agreement ends is how a client
changes provider without downtime: the new primary receives the data and
signs a checkpoint while the old one is still bound.

**Adding a replica provider (optional, permissionless):**
1. Provider quotes replica `AgreementTerms` (with `replica_params`: sync
   funding and interval) off-chain and signs them
2. Anyone calls `add_replica_provider` with the signed terms on an
   existing bucket → `StorageAgreement` created with `ProviderRole::Replica`
3. Replica syncs data autonomously from primaries, other replicas, or any data
   holder willing to push it (everything is content-addressed and self-verifying)
4. Replica calls `confirm_replica_sync` on-chain → receives per-sync payment, becomes challengeable

**Binding contract:**

Once established, agreements are binding for both parties until expiry:
- **No early exit for providers**: Providers cannot voluntarily leave. They committed to store data for the agreed duration.
- **No early cancellation for clients**: Clients cannot cancel and reclaim locked payment. They committed to pay for the agreed duration.
- **Provider's protection**: Providers author every quote they sign — owner, quota, duration and a `valid_until` expiry — and the price, sync price and stake applied at redemption are their own posted terms, so nothing binds them that they didn't explicitly offer. They can also block future extensions via `set_extensions_blocked`.
- **Client's protection**: Clients can challenge if provider loses data (slashing). At settlement, clients can burn payment to signal poor service (burns cost an additional premium, making them a credible but costly signal).

**Agreement expiry:**

When `expires_at` is reached:
1. Provider calls `claim_expired_agreement` to receive payment, OR
2. Client calls `end_agreement` with pay/burn decision within settlement window
3. Provider is no longer bound to store data
4. Provider won't be included in future checkpoints

**Bucket without providers:** ending the last agreement removes the provider
from `bucket.primary_providers` and deletes the agreement, not the bucket.
The bucket, its members, `snapshot` and `frozen_start_seq` remain on-chain.
The admin can add a new primary with `add_primary_provider` at any later
time and re-upload the data; the `bucket_id` and every external reference to
it stay valid.

**Snapshot liability**: Providers remain liable for snapshots they signed until those snapshots are superseded by a new checkpoint that doesn't include them, or until the bucket's canonical depth grows past the data they signed for.

### Multi-Provider Coordination (Primary Providers)

Primary providers don't sync with each other. Clients are responsible for uploading to each primary provider they want to store their data.

**Flow**:
1. Client uploads data to Primary A, B, C (separately)
2. Client triggers commit on each provider, collects signatures
3. Client checkpoints on-chain with collected signatures
4. Primaries not in the snapshot should sync (client re-uploads)
5. After checkpoint, providers can prune non-canonical roots

**The client moves the data**: a primary added to a bucket that already has
data receives that data from the client, not from the chain and not from the
other providers. The client uploads the data to the new primary and includes
the new primary's signature in the next checkpoint. It first downloads the data
from a provider that has it, unless it still keeps a local copy; changing
provider then costs a full download and a full upload.

A provider-to-provider fetch could remove that download, but it does not
replace the client path: only the client sees which provider failed a transfer.
See "Provider-to-Provider Fetch" in the design doc's Future Directions.

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
    /// Currency for payments and staking. Funds are immobilised with
    /// `fungible` holds (see "Funds on hold" below), never `reserve`.
    type Currency: Mutate<Self::AccountId>
        + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
        + BalancedHold<Self::AccountId>;

    /// The runtime's overarching hold reason.
    type RuntimeHoldReason: From<HoldReason>;

    /// Treasury account to receive burned payments.
    type Treasury: Get<Self::AccountId>;

    /// Maximum length of provider multiaddr.
    #[pallet::constant]
    type MaxMultiaddrLength: Get<u32>;

    /// Maximum members per bucket.
    #[pallet::constant]
    type MaxMembers: Get<u32>;

    /// Maximum physical signers per bucket across its primary agreements — a
    /// virtual primary counts its members. Bounds the checkpoint bitfield and
    /// the signatures verified per checkpoint.
    #[pallet::constant]
    type MaxPrimarySlots: Get<u32>;

    /// Maximum members of a virtual provider (virtual-provider extension);
    /// `<= MaxPrimarySlots`.
    #[pallet::constant]
    type MaxPhysicalMembers: Get<u32>;

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

    /// Maximum validity window of a provider-signed terms quote: redemption
    /// requires `terms.valid_until <= now + RequestTimeout`.
    #[pallet::constant]
    type RequestTimeout: Get<BlockNumberFor<Self>>;

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
| `MaxMultiaddrLength` | `128` |
| `MaxMembers` | `100` |
| `MaxPrimarySlots` | `8` |
| `MaxPhysicalMembers` | `4` |
| `MaxChunkSize` | `262_144` (256 KiB) |
| `ChallengeTimeout` | `48 * RC_HOURS` |
| `SettlementTimeout` | `48 * RC_HOURS` |
| `RequestTimeout` | `6 * RC_HOURS` |
| `MaxChallengesPerDeadline` | `1_000` |
| `AnchorBlockTimeMillis` | `6_000` |
| `Treasury` | derived from `PalletId(*b"py/trsry")` |

### Funds on Hold

Funds are immobilised with `fungible` **holds** under a tagged reason, so the
claims stay separable on one account and `try_state` can check them against the
pallet's bookkeeping:

```rust
#[pallet::composite_enum]
pub enum HoldReason {
    /// Provider collateral. The only hold that is ever slashed.
    ProviderStake,
    /// An agreement's prepaid fee, held on its owner (plus, for replicas,
    /// the sync balance) until settlement.
    AgreementPayment,
    /// A challenger's anti-spam deposit, refunded on resolution minus the
    /// provider's response-cost share.
    ChallengeDeposit,
}
```

An agreement's escrow always sits on its **owner** (permissionless top-ups move
a third party's funds there first, since settlement pays out of the owner's
hold), and — unlike `reserve` — a hold must leave the existential deposit
spendable, so registering needs `stake + ED` of free balance and an account
with a hold cannot be reaped.

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
    /// are supported: 32 bytes for Sr25519/Ed25519, 33 for compressed
    /// Ecdsa/Eth. The 64-byte capacity is reserved for future schemes;
    /// registration currently rejects anything but 32 or 33 bytes.
    pub public_key: BoundedVec<u8, ConstU32<64>>,
    /// Stake used for *new* agreements. Existing agreements snapshotted their
    /// own stake at creation, so this can be lowered without affecting them —
    /// the actually-locked amount is `locked_stake()` (see "Changeable Stake").
    pub stake: BalanceOf<T>,
    /// Grow-only max expiry over live agreements struck at the current `stake`
    /// generation. Bumped on every agreement create; never decremented. Folded
    /// into `higher_stake_lock` when `stake` is lowered, then reset to zero.
    pub cur_until: BlockNumberFor<T>,
    /// A previous, higher stake generation still owed to live agreements.
    /// Blocks a further lowering until `now >= until`. Set
    /// only by `set_stake` when lowering (see "Changeable Stake").
    pub higher_stake_lock: StakeLock<BalanceOf<T>, BlockNumberFor<T>>,
    /// Total contracted bytes (sum of max_bytes across all agreements).
    /// The *only* verifiable capacity figure — the stake/bytes invariant is
    /// enforced against this, not the self-declared `max_capacity`.
    pub committed_bytes: u64,
    /// Pins the provider-side terms an agreement is struck against. Every call
    /// that binds an owner to the provider's terms — the three redemption calls,
    /// `extend_agreement`, `top_up_agreement` and `create_replacement` — takes the client's
    /// `expected_version` and fails if it has moved. This is what removes the
    /// need for a `max_payment` bound. Bumped on the worse-direction change to a term the
    /// quote/request doesn't carry explicitly: **price ↑**, **replica sync price
    /// ↑**, **stake ↓**, or (virtual providers) a member leaving. Duration,
    /// capacity and `accepting_*` are checked directly against the quote's
    /// `max_bytes`/`duration`, and strictly-better changes never bump. See
    /// "Term Pinning".
    pub version: u32,
    /// Provider settings
    pub settings: ProviderSettings<T>,
    /// Provider statistics - clients use these to evaluate quality
    pub stats: ProviderStats<T>,
}

/// A higher, previous stake generation still owed to live agreements.
/// Default `{ stake: 0, until: 0 }` = none.
pub struct StakeLock<Balance, BlockNumber> {
    /// The higher stake figure that stays locked.
    pub stake: Balance,
    /// Block until which it is owed; lowering is blocked until `now >= until`.
    pub until: BlockNumber,
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
    /// Total amount clients burned in total
    pub amount_burned: BalanceOf<T>,
    /// Agreements the provider refunded (`refund_agreement`): an admitted
    /// failure to serve, milder than a burn
    pub agreements_refunded: u32,
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
    /// Total payment ever received by this provider for storage service:
    /// agreement settlements, extension payments, and replica sync
    /// payments. Monotonically increasing, never reset by a slash or
    /// anything else. A historical record, not a live balance.
    pub lifetime_revenue: BalanceOf<T>,
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
    /// Self-declared advisory capacity ceiling in bytes. `0` means unlimited.
    /// When non-zero, the provider will not accept agreements that push
    /// `committed_bytes` past it. This is a courtesy signal only — the provider
    /// sets it and could misreport it. No stake check is tied to it, nor to
    /// `committed_bytes` (see "Stake vs. capacity").
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
    /// Next agreement id to assign, incremented on each agreement created.
    /// Makes every agreement in this bucket uniquely identifiable over time so a
    /// commitment binds to a specific agreement (see `StorageAgreement.agreement_id`).
    pub next_agreement_id: u64,
    /// Read visibility (see `Visibility`). On-chain, only the challenge
    /// extrinsics read it: `Private` restricts primary challenges to members
    /// and primary-agreement owners.
    pub visibility: Visibility,
    /// If Some, bucket is append-only from this start_seq.
    /// Checkpoints with start_seq < frozen_start_seq are rejected (prevents deletions).
    pub frozen_start_seq: Option<u64>,
    /// Minimum signing slots required for a checkpoint. Bounded by the bucket's
    /// slot layout; clamped down, with an event, when the layout shrinks.
    pub min_providers: u32,
    /// Primary provider account IDs. Each expands to one checkpoint slot, or to
    /// one per member for a virtual provider; the expansion is bounded by
    /// `T::MaxPrimarySlots`. These are admin-controlled providers that:
    /// - Receive data directly from writers
    /// - Count toward min_providers for checkpoints
    /// Stored inline for efficient checkpoint reads (one storage access).
    pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimarySlots>,
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
    /// Bitfield over the bucket's slot layout: `primary_providers` expanded in
    /// order, a virtual provider to its snapshotted members. Bit i (LSB0) is set
    /// if slot i signed. A virtual's members enter only as a group of at least
    /// its threshold `k` per call (virtual-provider extension, "Checkpoints"),
    /// so a set bit always means a liable signer.
    /// Stored as `Vec<u8>` with explicit `count_signers()` / `has_provider_signed()`
    /// helpers rather than `BitVec` to keep encoding stable and `no_std`-friendly.
    /// The layout is bounded by `T::MaxPrimarySlots`, so indices are stable
    /// within a checkpoint; if it changes between checkpoints (a primary added or
    /// removed, a virtual's member set re-snapshotted) the bits are adjusted in
    /// place on that extrinsic.
    pub primary_signers: Vec<u8>,
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
    /// Per-bucket unique id (from `Bucket.next_agreement_id`). Commitments name
    /// it (`CommitmentPayload.agreement_id`) so a commitment is only valid under
    /// the agreement it was made for; when the agreement ends, its commitments
    /// are void — a re-registering provider can't be challenged on obsolete
    /// state, and off-chain commitments have a definite end of life.
    pub agreement_id: u64,
    /// Who owns this agreement (can top up quota, transfer ownership)
    pub owner: T::AccountId,
    /// Maximum bytes (quota) — provider accepts uploads up to this
    pub max_bytes: u64,
    /// Payment locked for storage (bytes * time). Prepaid at creation/extension
    /// from the price then in force; the price itself is not stored — nothing
    /// reads it after payment is computed (extension recomputes at the *current*
    /// price, gated by the version pin).
    pub payment_locked: BalanceOf<T>,
    /// Provider stake snapshotted at creation/extension. The provider stays
    /// liable at this figure until the agreement ends, independent of later
    /// stake changes (see "Changeable Stake").
    pub stake: BalanceOf<T>,
    /// Agreement expiration
    pub expires_at: BlockNumberFor<T>,
    /// Whether provider has blocked extensions for this specific agreement
    pub extensions_blocked: bool,
    /// Provider role for this bucket.
    pub role: ProviderRole<T>,
    /// Block when agreement became active (for statistics)
    pub started_at: BlockNumberFor<T>,
    /// Owner-created successor, not yet live ("Replacement agreements").
    pub pending_replacement: Option<PendingReplacement<T>>,
}

#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub enum ProviderRole<T: Config> {
    /// Receives data directly from writers.
    /// - Admin-controlled (stored in bucket.primary_providers)
    /// - Count toward min_providers for checkpoints
    Primary,
    /// Syncs data from other providers autonomously.
    /// - Permissionless (anyone can add)
    /// - Does NOT count toward min_providers
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
        /// For a virtual provider: which members of the agreement's snapshot
        /// signed `last_sync` (bitmask over the snapshot); they are the ones a
        /// failed `challenge_replica` slashes. Unused for a physical provider.
        last_sync_signers: u8,
    },
}

/// Defined in `storage_primitives`: the off-chain quote a provider signs and
/// the owner redeems on-chain (see `create_bucket_with_primary` /
/// `add_primary_provider` / `add_replica_provider`). The provider signs
/// `blake2_256(context | SCALE(terms))`, where `context` is
/// `PRIMARY_TERM_CONTEXT` (`"primary-term-v2:"`) or `REPLICA_TERM_CONTEXT`
/// (`"replica-term-v2:"`) — domain separation between the two flavours.
///
/// The quote is a **consent token**, not a price carrier: it names *who* may
/// redeem, *how much* and *how long*. Every provider-side term — price,
/// replica sync price, stake (and, for virtual providers, composition) — is
/// read from `ProviderInfo` at redemption and guarded by the client's
/// `expected_version`, so there is exactly one source for those figures (see
/// "Term Pinning").
pub struct AgreementTerms<AccountId, Balance, BlockNumber> {
    /// Owner that will be bound by these terms (must match the extrinsic
    /// origin at redemption).
    pub owner: AccountId,
    /// Storage quota committed by the provider, in bytes.
    pub max_bytes: u64,
    /// Agreement duration in blocks from activation.
    pub duration: BlockNumber,
    /// Block number after which the quote is no longer redeemable.
    pub valid_until: BlockNumber,
    /// Replay-protection nonce, supplied by the owner in the quote request:
    /// must equal the owner's next expected value (`AgreementNonces`), so a
    /// signed quote is redeemable at most once.
    pub nonce: u64,
    /// Bucket the quote is for (see `BucketTarget` below).
    pub bucket: BucketTarget,
    /// `None` for primary terms; `Some(_)` for replica terms, carrying the
    /// per-sync funding parameters.
    pub replica_params: Option<ReplicaTerms<Balance, BlockNumber>>,
}

/// Which bucket a quote is for. Signed as part of the terms, so a provider
/// can decline one bucket without declining the account.
pub enum BucketTarget {
    /// A bucket created when the quote is redeemed. Only
    /// `create_bucket_with_primary` accepts it.
    New,
    /// An existing bucket. Must equal the `bucket_id` the redeeming call
    /// targets; only `add_primary_provider` and `add_replica_provider`
    /// accept it.
    Existing(BucketId),
}

/// Replica-specific parameters of a signed quote. The per-sync price is *not*
/// here — it is `provider.settings.replica_sync_price` at redemption, pinned
/// by the client's `expected_version`, and snapshotted into
/// `ProviderRole::Replica.sync_price`.
pub struct ReplicaTerms<Balance, BlockNumber> {
    /// Balance held on the owner to fund per-sync confirmations. The
    /// pallet draws down the snapshotted `sync_price` from this on each
    /// accepted sync.
    pub sync_balance: Balance,
    /// Minimum blocks between sync confirmations the provider commits to.
    /// 0 means no time-based limit (only "new root" check applies).
    pub min_sync_interval: BlockNumber,
}

/// Next expected `AgreementTerms.nonce` for this owner. Redemption requires an
/// exact match (`NonceMismatch`) and advances the counter by one, so a signed
/// quote is redeemable at most once, in the order it was requested. Keyed by
/// owner, not provider, so replay protection does not depend on a provider's
/// registration and nothing has to outlive deregistration.
#[pallet::storage]
pub type AgreementNonces<T: Config> =
    StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

/// Pending challenges, keyed by (deadline anchor block, per-deadline index).
/// At most `MaxChallengesPerDeadline` challenges share a deadline; expired
/// deadlines are drained by the `on_initialize` slash sweep.
#[pallet::storage]
pub type Challenges<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, BlockNumberFor<T>, // deadline (anchor block)
    Twox64Concat, u16,                   // index within the deadline
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

/// Number of unresolved challenges outstanding against a specific
/// `(bucket, provider)` pair. Incremented in `create_challenge` and
/// decremented exactly once per resolution (defended/invalid-response in
/// `respond_to_challenge`, or timeout in the `on_initialize` sweep). Gates
/// that agreement's teardown (`end_agreement`, `claim_expired_agreement`,
/// `cleanup_bucket_internal`): an agreement — and with it the provider's
/// `committed_bytes` — cannot be released out from under a live challenge.
/// Together with "no live agreement ⇒ not challengeable" this is what makes
/// one-step `deregister_provider` safe: the stake can only be withdrawn once
/// every agreement has ended, and no agreement can end while slashable.
#[pallet::storage]
pub type PendingChallengesByBucket<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, BucketId,
    Blake2_128Concat, T::AccountId,
    u32,
    ValueQuery,
>;

/// Reverse index: account → buckets it is a member of. Set-membership via key
/// presence, so an account can be in **unbounded** buckets (state cost is the
/// only limit — no artificial per-account cap). Maintained on every membership
/// change; read only by the `member_buckets` runtime API (paged via
/// `iter_prefix`) and `try_state`.
///
/// **Convenience index.** It exists only to answer "which buckets is this
/// account in / does this provider serve" cheaply on-chain. If good off-chain
/// indexing is available, this can be dropped and the query served there; the
/// runtime API is versioned so it can be deprecated. See "Reverse indexes".
#[pallet::storage]
pub type MemberBuckets<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, T::AccountId,
    Blake2_128Concat, BucketId,
    (),
    ValueQuery,
>;

/// Reverse index: provider → buckets it has an agreement in. Same rationale as
/// `MemberBuckets` — `StorageAgreements` is keyed bucket-first, so "which
/// buckets does provider P serve" would otherwise be a full scan. Set-membership
/// via key presence (unbounded), maintained on agreement create/end, read by the
/// `provider_buckets` runtime API. Also a convenience index (see above).
#[pallet::storage]
pub type ProviderBuckets<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, T::AccountId,
    Blake2_128Concat, BucketId,
    (),
    ValueQuery,
>;
```

### Changeable Stake

Stake is **not** grow-only. A provider may raise it any time, and lower it in a
way that never weakens an agreement already struck: **each agreement snapshots
the stake in force when it was created**, and a provider stays liable at that
figure until the agreement ends. `provider.stake` is only the figure used for
*new* agreements. (Price needs no such snapshot — it is prepaid at creation, so
nothing reads it later.)

The actually-locked amount is computed without ever iterating agreements:

```rust
fn locked_stake(p: &ProviderInfo) -> Balance {
    let lock = &p.higher_stake_lock;
    if now() < lock.until { max(p.stake, lock.stake) } else { p.stake }
}
```

Maintenance is O(1) per event, no scan:

- **create agreement** (expiry `E`): `cur_until = max(cur_until, E)`. (New agreements
  always use the current `stake`, so their snapshot is `provider.stake`.)
- **raise stake:** set `stake`; nothing else. Old agreements are now *below* the
  new figure, so `locked_stake` already covers them.
- **lower stake** to `X` (`set_stake`): allowed **only if `higher_stake_lock` has
  expired** (`now >= higher_stake_lock.until`; trivially true for the default).
  Then fold the current generation into it —
  `higher_stake_lock = { stake, until: cur_until }` — reset `cur_until = 0` (technically not needed, but good to keep intent: current gen's max), and
  set `stake = X`. The old (higher) figure stays locked until `cur_until`, the
  latest expiry of any agreement struck under it.
- **agreement end:** nothing.

This deliberately **overshoots** rather than track exact per-agreement maxima
(which would need an unbounded scan on end): while `higher_stake_lock` is live it
locks the whole previous generation at its top stake for its longest expiry, even
agreements that were actually cheaper or shorter. Since providers lower stake
rarely, the over-lock is a small, bounded cost for O(1) accounting. The "can't
lower again while a higher generation is still owed" rule is what keeps both
`cur_until` and `higher_stake_lock` grow-only between resets, so neither ever
needs a decrement.

### Stake vs. capacity

**The chain enforces no relation between stake and capacity.** `max_capacity` is
self-declared and unverifiable, so it is advisory only (a "not accepting past
here" hint). `committed_bytes` — the sum of `max_bytes` over agreements the
provider accepted — is verifiable, but a stake-per-byte constraint on it
(`stake >= committed_bytes * MinStakePerByte`) would enforce nothing real.

`committed_bytes` remains as an informative figure — clients read it to judge
how loaded a provider is, and deregistration requires it to reach zero.

### Replacement agreements

The owner-only path that ends an agreement early. `create_replacement` stores a
pending successor in the agreement record (`StorageAgreement.pending_replacement`):
a fresh `agreement_id`, the provider's current terms — for a virtual provider its
current member set and `per_provider_stake` — a duration, and the new payment,
held. Pending means not live: no liability, commitments naming it are not valid,
no checkpoint slots. The old agreement runs on unchanged.

```rust
pub struct PendingReplacement<T: Config> {
    pub agreement_id: u64,
    /// Provider terms at creation; for a virtual provider includes its member
    /// set and `per_provider_stake` (virtual-provider extension).
    pub terms: AgreementTerms<T>,
    pub duration: BlockNumberFor<T>,
    pub payment_locked: BalanceOf<T>,
}
```

The first `checkpoint` carrying the successor's signature — for a virtual
provider, at least `k` of its members' — **activates** it: the record becomes the
new agreement with `expires_at = now + duration` (bumping `cur_until` as any
agreement creation does), and the old one settles exactly
as `extend_agreement` step 1 does — elapsed period paid to the old provider (a
virtual's snapshotted members, equal split), unelapsed remainder rolled into the
successor's escrow. Activation requires the old agreement to be live; if it
expires first, the pending successor ends with it and settles as an expired
agreement in the same call — payment to the provider, or burned by the owner.
For a virtual
provider, activation also raises the `until` of any snapshotted member that has
since left (virtual-provider extension, "Changing a live agreement's member
set").

Owner-only because activation spends the owner's escrow. A third party keeping a
frozen bucket alive funds its own replica instead (design doc "Permissionless
persistence"). The virtual-provider extension uses replacements to swap a
member set without a gap in the client's guarantee.

### Term Pinning (no-surprise agreements)

The goal is to stop an agreement landing on worse terms than the client evaluated
(the read→submit race) — the job `max_payment` used to do for price, generalized.
A quote already carries `max_bytes` and `duration`, so mismatches on duration
limits, capacity, or `accepting_*` make it fail on its own — no version needed.
The provider-side terms a client relied on but that the quote does *not* carry
are **price** (payment is computed from the provider's current price — the
original race), **replica sync price**, **stake** (it picked the provider for its
backing), and — for a virtual provider — its **composition** (a member leaving
drops redundancy, e.g. `3`-of-`4` → `3`-of-`3`, even at unchanged `stake`; see the
virtual-provider extension). So `ProviderInfo.version` bumps on `price ↑`,
`replica_sync_price ↑`, `stake ↓`, or a virtual member leaving; strictly-better
changes (price ↓, stake ↑, a member joining) never bump.

Deliberately, **none of those figures travel inside the signed quote** — they are
read from `ProviderInfo` at redemption. Snapshotting some terms from the provider
and carrying others in the quote would give two sources of truth for what an
agreement was struck against; the version pin makes a single source sufficient.

The pin is the client's, and it is checked the same way everywhere. Every call
that binds an owner to the provider's terms — `create_bucket_with_primary`,
`add_primary_provider`, `add_replica_provider`, `extend_agreement`,
`top_up_agreement`, `create_replacement` — takes the version at which the client read those terms as
`expected_version` and fails with `ProviderVersionMismatch` if the current
`version` differs. The quote carries no version: the provider gains nothing from
signing one, since whatever is applied at redemption is its own posted terms,
and a client-supplied pin is enforced by the chain rather than depending on the
client re-checking a provider-supplied value. A provider that worsens its terms
after quoting still voids its outstanding quotes — the bump fails the client's
pin. This closes the race where terms worsen between the client reading them and
its extrinsic landing, and because it pins the price a separate `max_payment`
bound is unnecessary anywhere.

The one-directional bump (worse-only) means a client isn't spuriously rejected
when the provider's terms got *better* between its read and submit (a price drop
or stake raise) — the pin only fires on a change it would actually care about.

### Provider Public Key & Signature Type

Providers register a raw public key alongside their multiaddr (32 bytes for
Sr25519/Ed25519, 33 bytes for compressed Ecdsa/Eth). All on-chain signature
verification uses `sp_runtime::MultiSignature` against this key, so a single
provider can use any of the supported schemes.

The ratified matrix — the signature's variant picks how the expected signer
account is derived from the registered key (via `MultiSigner::into_account()`):

| Scheme    | Registered key      | Message digest | Expected signer account                                              |
| --------- | ------------------- | -------------- | -------------------------------------------------------------------- |
| `Sr25519` | 32 raw bytes        | none           | the key bytes as `AccountId32`                                        |
| `Ed25519` | 32 raw bytes        | none           | the key bytes as `AccountId32`                                        |
| `Ecdsa`   | 33 bytes compressed | blake2-256     | `blake2_256(key)`                                                     |
| `Eth`     | 33 bytes compressed | keccak-256     | `keccak_256(key)[12..]` in a `0xEE`-filled `AccountId32` (the `pallet_revive` address-mapping convention — Ethereum-wallet tooling) |

Registration accepts only 32- and 33-byte keys. The `BoundedVec` keeps 64
bytes of capacity reserved for future schemes, but no supported scheme
verifies against a longer key, so any other length is rejected at
registration (`InvalidPublicKey`) instead of registering a provider that
could never pass verification.

The provider node signs with any of the four schemes (`--key-scheme`,
default sr25519) and emits every signature as SCALE-encoded `MultiSignature`
hex, so the scheme tag travels with the signature on every wire path.

Besides the quote (`AgreementTerms`, signed with a flavour context prefix), two
on-chain signed payloads exist (all SCALE-encoded, all carry an explicit
`version: u8` so the protocol can evolve without breaking existing signatures):

- `CommitmentPayload { version, bucket_id, agreement_id, commitment }` —
  what providers sign for `commit`, `checkpoint`, `extend_checkpoint`, and
  `challenge_offchain` (`commitment: Commitment` is defined in [Data
  Structures](#data-structures)). For `challenge_offchain` the challenger
  passes the signed `commitment` and `agreement_id` through unchanged so the
  pallet's payload reconstruction matches the signature; the challenge is
  rejected if that agreement is no longer live. `extend_checkpoint` reconstructs
  each late signer's payload the same way — from the snapshot's `commitment` and
  that signer's live `agreement_id` — so no stored per-signer disambiguator is
  needed.
- The replica sync `roots` array (`[Option<H256>; 7]`) — signed for
  `confirm_replica_sync` to attest which roots the replica actually has.

**Provider signatures.** Every call that takes one provider's signature over
the quote or one of these payloads takes a signature set, so the same call
serves physical and virtual providers:

```rust
/// Signatures on behalf of one provider.
pub type ProviderSignatures<T> =
    BoundedVec<(T::AccountId, Signature), T::MaxPhysicalMembers>;
```

The rule, one verification helper for all calls: for a physical provider,
exactly one entry, signed by the provider's registered key; for a virtual
provider, at least `k` distinct members of the call's member set, each verified
against that member's own key, fewer rejects the call (virtual-provider
extension). The member set is the live `members` for the three redemption calls
and the agreement's snapshot for `challenge_offchain` and
`confirm_replica_sync`.

Three calls take signatures of several providers or add to an existing set, and
so use flat `(AccountId, Signature)` pairs instead:

- `checkpoint` — pairs for the whole slot layout; per provider the rule above,
  a virtual primary's pairs all-or-nothing (≥`k` or none).
- `extend_checkpoint` — per provider either a whole group at ≥`k`, or further
  members of a virtual group that already reached `k` in the current snapshot.
- `extend_challenge` — further members of the challenged virtual provider's
  snapshot, added to a challenge that already has ≥`k`.

**Replay & commitment validity.** A commitment is bound to one `agreement_id` and
is valid only while that agreement is live; when the agreement ends the commitment
is void. That is the whole replay model — there is no time-based nonce or recency
window. A provider stays responsible for what it signed exactly as it stays
responsible for the data: to delete data it must hold the admin-signed deletion
commitment (the `Deleted` defense), and keeping that evidence is its own duty,
just like keeping the data. Losing it is self-inflicted, no different from losing
the data — not a replay the protocol guards against by expiring signatures.

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
    /// Deregistration: stake returned, provider entry removed.
    ProviderDeregistered {
        provider: T::AccountId,
        stake_returned: BalanceOf<T>,
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
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
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
    /// An admin changed who may read the bucket.
    BucketVisibilityChanged {
        bucket_id: BucketId,
        visibility: Visibility,
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
    /// Emitted by `create_bucket_with_primary` and `add_primary_provider`,
    /// together with `StorageAgreementEstablished`.
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
    
    /// Owner redeemed provider-signed primary terms; bucket created and
    /// agreement opened atomically.
    StorageAgreementEstablished {
        bucket_id: BucketId,
        agreement_id: u64,
        provider: T::AccountId,
        owner: T::AccountId,
        terms: AgreementTerms<T>,
        payment_locked: BalanceOf<T>,
        expires_at: BlockNumberFor<T>,
    },
    /// Owner redeemed provider-signed replica terms against an existing bucket.
    ReplicaAgreementEstablished {
        bucket_id: BucketId,
        agreement_id: u64,
        provider: T::AccountId,
        owner: T::AccountId,
        terms: AgreementTerms<T>,
        payment_locked: BalanceOf<T>,
        expires_at: BlockNumberFor<T>,
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
        escrow: BalanceOf<T>,
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
    AgreementRefunded {
        bucket_id: BucketId,
        provider: T::AccountId,
        refunded: BalanceOf<T>,
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
        /// Timeout, or which response type failed verification (see `SlashReason`)
        reason: SlashReason,
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
        /// Paginated list of all registered providers. Discovery filters and
        /// ranks off-chain from these pages (see below).
        fn providers(offset: u32, limit: u32) -> Vec<(AccountId, ProviderInfoResponse)>;

        // ── Buckets ───────────────────────────────────────────────────────
        fn bucket_info(bucket_id: BucketId) -> Option<BucketResponse>;
        fn bucket_ids(offset: u32, limit: u32) -> Vec<BucketId>;
        fn bucket_providers(bucket_id: BucketId) -> Vec<AccountId>;

        // ── Agreements ────────────────────────────────────────────────────
        fn agreement_info(bucket_id: BucketId, provider: AccountId) -> Option<AgreementResponse>;
        fn bucket_agreements(bucket_id: BucketId) -> Vec<AgreementResponse>;
        /// "Which buckets do I serve." Paged; backed by the `ProviderBuckets`
        /// reverse index (not a full scan of `StorageAgreements`).
        fn provider_agreements(provider: AccountId, offset: u32, limit: u32) -> Vec<AgreementResponse>;

        // ── Reverse lookups (convenience — see "Reverse indexes") ──────────
        /// "Which buckets is this account a member of." Paged; backed by
        /// `MemberBuckets`.
        fn member_buckets(account: AccountId, offset: u32, limit: u32) -> Vec<BucketId>;
        /// "Which buckets does this provider have an agreement in." Paged;
        /// backed by `ProviderBuckets`.
        fn provider_buckets(provider: AccountId, offset: u32, limit: u32) -> Vec<BucketId>;

        // ── Challenges ────────────────────────────────────────────────────
        fn challenges_at(block: BlockNumber) -> Vec<ChallengeResponse>;
        fn bucket_challenges(bucket_id: BucketId) -> Vec<ChallengeResponse>;
        fn provider_challenges(provider: AccountId) -> Vec<ChallengeResponse>;
        fn challenger_challenges(challenger: AccountId) -> Vec<ChallengeResponse>;
    }
}
```

**Reverse indexes & client reads.** `MemberBuckets` and `ProviderBuckets` are
**convenience** reverse indexes that let the runtime API answer "which buckets is
this account in / does this provider serve" without scanning. They exist because
the primary maps are keyed the other way (`StorageAgreements` is bucket-first).
Clients must reach them **only through the versioned runtime API above — never by
raw storage query** — so the interface can evolve, be paginated, and be
**deprecated** once good off-chain indexing exists (at which point the indexes
themselves can be dropped). They are set-membership double-maps (unbounded per
account; state cost is the only limit), maintained on every membership/agreement
change, and never iterated on-chain in extrinsics (only in `try_state`).

**Discovery is off-chain.** The runtime API offers no provider search: ranking
providers by price, capacity, duration or reputation is marketplace policy, and
a search inside the runtime would scan every provider on each call, on whatever
node answers the RPC. Clients (the SDK, an indexer) page `providers` and filter
and rank themselves; changing the ranking then needs no runtime upgrade. The
same holds for challengers choosing whom to challenge: they rank from the same
pages and the providers' `stats`.

Response types live in `crates/pallets/storage-provider/src/runtime_api.rs`
(`ProviderInfoResponse`, `BucketResponse`, `AgreementResponse`,
`ChallengeResponse`, …): dedicated structs rather than the storage types, so the
storage layout can change without breaking clients. They are generic over the
API's `AccountId`, `Balance` and `BlockNumber`, as upstream runtime APIs are
(`AccountNonceApi`, `NominationPoolsApi`, `RuntimeDispatchInfo`); the runtime
metadata exposes the concrete types, so PAPI and subxt produce typed bindings.
`ProviderInfoResponse` groups its historical counters under a nested
`stats: ProviderStatsInfo`, apart from settings and connection info.

`ProviderInfoResponse` reports **`version`** (so a client can pin it in the
agreement request — see "Term Pinning") and both `stake` (the figure for new
agreements) and `locked_stake` (the currently-reserved amount, ≥ `stake` while a
higher generation is still owed — see "Changeable Stake").

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
    ///   compressed Ecdsa/Eth; other lengths are rejected (the 64-byte
    ///   capacity stays reserved for future schemes). Used to verify provider
    ///   signatures (commitments, checkpoints, replica sync) on-chain.
    /// - `stake`: Initial stake to lock (must meet minimum, provides sybil resistance)
    ///
    /// Initialises `cur_until = 0`, `higher_stake_lock = { stake: 0, until: 0 }`,
    /// `version = 0`.
    #[pallet::weight(...)]
    pub fn register_provider(
        origin: OriginFor<T>,
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        public_key: BoundedVec<u8, ConstU32<64>>,
        stake: BalanceOf<T>,
    ) -> DispatchResult;

    /// Set the provider's stake for *new* agreements (raise or lower).
    ///
    /// Raising takes effect immediately. Lowering is allowed only when no higher
    /// stake generation is still owed (`higher_stake_lock` expired, i.e.
    /// `now >= higher_stake_lock.until`) and the new value is `>= MinProviderStake`;
    /// otherwise `StakeStillLocked` / `InsufficientStakeForCommitted`. Existing
    /// agreements keep the stake they snapshotted — see "Changeable Stake".
    /// Bumps `version` only when lowering (a raise is strictly better — see
    /// "Term Pinning").
    ///
    /// Parameters:
    /// - `new_stake`: Stake to use for future agreements.
    #[pallet::weight(...)]
    pub fn set_stake(
        origin: OriginFor<T>,
        new_stake: BalanceOf<T>,
    ) -> DispatchResult;

    /// Deregister and withdraw stake.
    ///
    /// Fails if `committed_bytes > 0`: a provider offboards by simply not
    /// accepting new agreements/extensions and letting its existing ones run to
    /// expiry. Liability is exactly "has an active agreement", so once the last
    /// one has ended the provider is no longer challengeable (all challenge
    /// paths reject a provider without a live agreement — see
    /// `challenge_checkpoint`) and its stake can be unreserved immediately. No
    /// announcement window is needed: there is no post-expiry challenge to race,
    /// and a challenge opened while an agreement was live keeps that agreement
    /// (hence `committed_bytes > 0`) alive until it resolves
    /// (`PendingChallengesByBucket`).
    ///
    /// Removes the `Providers` entry. Quote replay protection is per owner
    /// (`AgreementNonces`), so a quote already redeemed stays unredeemable
    /// against a later re-registration of the same key without any retained
    /// provider state.
    #[pallet::weight(...)]
    pub fn deregister_provider(origin: OriginFor<T>) -> DispatchResult;

    /// Update provider settings.
    /// 
    /// Allows provider to change pricing, duration limits, capacity, and
    /// availability. Changes apply to new agreements only; existing agreements
    /// retain their locked terms.
    ///
    /// Validation:
    /// - `min_duration <= max_duration` (`MinDurationExceedsMaxDuration`).
    /// - If `max_capacity > 0`: must be `>= committed_bytes`
    ///   (`CapacityBelowCommitted`). `max_capacity` is a self-declared *advisory*
    ///   ceiling only.
    /// - Bumps `version` iff `price_per_byte` or `replica_sync_price` increased
    ///   (`None` → `Some` counts as an increase). Other settings (durations,
    ///   capacity, `accepting_*`) are checked directly against a quote's
    ///   params, so they need no version bump (see "Term Pinning").
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
    /// capacity, accepting flags) stays the same. Does **not** bump `version`:
    /// the endpoint is not a term of the agreement (same provider, same
    /// economics), so it must not invalidate in-flight agreement requests.
    #[pallet::weight(...)]
    pub fn update_provider_multiaddr(
        origin: OriginFor<T>,
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
    ) -> DispatchResult;

    /// Block or unblock extensions for a specific bucket (provider only).
    /// Allows provider to stop a specific bucket from extending while
    /// continuing to accept extensions from other buckets.
    ///
    /// Requires a registered provider (`ProviderNotFound`) with a live
    /// agreement on the bucket (`AgreementNotFound`, `AgreementExpired`).
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
    /// Self-demotion (and self-removal via `remove_member`) is refused for
    /// the bucket's only admin (`LastAdminCannotBeRemoved`): a bucket always
    /// keeps ≥ 1 admin.
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
    /// - Remove themselves (refused for the bucket's only admin,
    ///   `LastAdminCannotBeRemoved` — a bucket always keeps ≥ 1 admin)
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

    // Agreements are established by redeeming provider-signed AgreementTerms
    // (see the storage section) — there is no on-chain request/accept
    // round-trip. The provider quotes and signs terms off-chain; the owner
    // submits them in a single call. `create_bucket_with_primary`,
    // `add_primary_provider` and `add_replica_provider` share the same
    // validation skeleton:
    // - `terms.bucket` must match the call (`TermsBucketMismatch`):
    //   `BucketTarget::New` for `create_bucket_with_primary`,
    //   `BucketTarget::Existing(bucket_id)` of the targeted bucket for
    //   `add_primary_provider` and `add_replica_provider`
    // - `add_primary_provider` and `add_replica_provider` also require that
    //   the bucket exists (`BucketNotFound`) and that no agreement already
    //   exists for this provider on it (`AgreementAlreadyExists`)
    // - `terms.owner` must match the origin (`TermsOwnerMismatch`)
    // - `terms.max_bytes > 0` (`InvalidMaxBytesRequest`)
    // - `now <= terms.valid_until <= now + T::RequestTimeout`
    //   (`TermsExpired` / `TermsValidityTooLong`)
    // - `sigs` must satisfy the provider signature rule (see
    //   `ProviderSignatures`) over `blake2_256(context | SCALE(terms))` with
    //   the flavour's context; for a virtual provider the member set is its
    //   live `members`, which the new agreement snapshots
    // - `terms.nonce` must equal the owner's next expected agreement nonce
    //   (`NonceMismatch`; see `AgreementNonces`)
    // - `expected_version == provider.version`
    //   (`ProviderVersionMismatch`; see "Term Pinning")
    // - the provider must be registered, within its duration bounds, and the
    //   added `terms.max_bytes` must fit its declared capacity
    //   (`CapacityExceeded`)
    //
    // Payment `provider.settings.price_per_byte * terms.max_bytes *
    // terms.duration` is held on the owner. The price is read from the
    // provider at redemption, not carried in the quote — the version pin is
    // the price protection, so there is no `max_payment` parameter. Likewise
    // the agreement snapshots `provider.stake` and (for replicas)
    // `provider.settings.replica_sync_price` as they stand at redemption.

    /// Redeem provider-signed primary terms: create a bucket and its first
    /// primary agreement in one atomic call. Equivalent to `create_bucket`
    /// followed by `add_primary_provider`, and emits the same events
    /// (`BucketCreated`, `ProviderAddedToBucket`,
    /// `StorageAgreementEstablished`).
    ///
    /// In addition to the shared checks above:
    /// - `terms.bucket` must be `BucketTarget::New` (`TermsBucketMismatch`).
    ///   This is the only call that accepts it; a quote for an existing
    ///   bucket cannot be redirected into a new one
    /// - `terms.replica_params` must be `None`
    /// - the provider must be accepting primaries
    ///   (`ProviderNotAcceptingPrimary`)
    ///
    /// The bucket is created with the owner as sole admin, `min_providers =
    /// 1` and the provider as its single primary. `visibility` sets the new
    /// bucket's read visibility; it is the owner's choice and not part of the
    /// provider-signed terms.
    #[pallet::weight(...)]
    pub fn create_bucket_with_primary(
        origin: OriginFor<T>,
        provider: T::AccountId,
        terms: AgreementTerms<T>,
        sigs: ProviderSignatures<T>,
        expected_version: u32,
        visibility: Visibility,
    ) -> DispatchResult;

    /// Redeem provider-signed primary terms: add a primary provider to an
    /// existing bucket (admin only).
    ///
    /// In addition to the shared checks above:
    /// - `terms.bucket` must be `BucketTarget::Existing(bucket_id)`
    ///   (`TermsBucketMismatch`)
    /// - the origin must be a bucket admin (`NotBucketAdmin`)
    /// - `terms.replica_params` must be `None`
    /// - the provider must be accepting primaries
    ///   (`ProviderNotAcceptingPrimary`)
    /// - adding the provider must keep the bucket's slot layout within
    ///   `T::MaxPrimarySlots` (`MaxPrimarySlotsExceeded`); a virtual provider
    ///   takes one slot per member
    ///
    /// Creates the `StorageAgreement` with `ProviderRole::Primary` and the
    /// admin as agreement owner, appends the provider to
    /// `bucket.primary_providers`, and emits `ProviderAddedToBucket` and
    /// `StorageAgreementEstablished`. The new primary is not in the current
    /// snapshot's signer bitfield until it signs a checkpoint; the client
    /// uploads the bucket's data to it (see Multi-Provider Coordination).
    ///
    /// Works on a bucket with zero primaries (all earlier agreements ended)
    /// and on a frozen bucket.
    #[pallet::weight(...)]
    pub fn add_primary_provider(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        terms: AgreementTerms<T>,
        sigs: ProviderSignatures<T>,
        expected_version: u32,
    ) -> DispatchResult;

    /// Redeem provider-signed replica terms: open a replica agreement on an
    /// existing bucket (anyone the provider quoted for can redeem).
    ///
    /// Creates a replica provider agreement:
    /// - Does NOT count toward min_providers for checkpoints
    /// - Syncs data autonomously from primaries or other replicas
    /// - Unlimited number of replicas per bucket
    ///
    /// No syncability check—a private bucket with zero replicas is accepted;
    /// an unfulfillable agreement is the funder's own risk. Rationale: design
    /// doc, "No on-chain gate on replica creation".
    ///
    /// The redeemer becomes the agreement owner (can top up, transfer
    /// ownership).
    ///
    /// In addition to the shared checks above:
    /// - `terms.bucket` must be `BucketTarget::Existing(bucket_id)`
    ///   (`TermsBucketMismatch`). The replica names the bucket it commits to
    ///   sync, so a quote cannot be redirected to a bucket it cannot read
    /// - `terms.replica_params` must be `Some(_)` (`MissingReplicaTerms`):
    ///   - `sync_balance`: Held on the owner on top of the storage
    ///     payment to fund per-sync payments at the provider's
    ///     `replica_sync_price` (read at redemption, snapshotted as
    ///     `sync_price`).
    ///     When exhausted, replica stops receiving sync payments but remains
    ///     bound until expiry. Can top up via `top_up_replica_sync_balance`.
    ///   - `min_sync_interval`: Minimum blocks between sync confirmations.
    ///     0 for no time-based limit.
    /// - the provider must be accepting replicas, i.e. have a
    ///   `replica_sync_price` set (`ProviderNotAcceptingReplicas`)
    #[pallet::weight(...)]
    pub fn add_replica_provider(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        terms: AgreementTerms<T>,
        sigs: ProviderSignatures<T>,
        expected_version: u32,
    ) -> DispatchResult;

    /// Top up quota for an existing agreement (owner only).
    /// Increases max_bytes, does not change duration.
    /// Actual payment = provider.price_per_byte * additional_bytes * remaining_duration.
    /// Fails with `ProviderVersionMismatch` if `expected_version != provider.version`
    /// (see "Term Pinning") — the pin replaces a `max_payment` bound here too.
    #[pallet::weight(...)]
    pub fn top_up_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        additional_bytes: u64,
        expected_version: u32,
    ) -> DispatchResult;

    /// Store a pending successor for a live agreement (**owner only**); see
    /// "Replacement agreements". Fails if one is already pending. The successor
    /// activates only while this agreement is live; otherwise it ends with it
    /// and settles as expired.
    pub fn create_replacement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        duration: BlockNumberFor<T>,
        expected_version: u32,
    ) -> DispatchResult;

    /// Extend agreement duration (**owner only**).
    /// Only while the agreement is live — an expired one settles via
    /// `end_agreement` / `claim_expired_agreement`, never here.
    /// 1. Settles current period: pays provider for elapsed time out of
    ///    escrow, capped at the agreement's `payment_locked`
    /// 2. Locks new payment for the extension at current provider terms
    /// 3. Updates end date to now + additional_duration
    /// 4. Re-snapshots the provider's current terms into the agreement
    ///
    /// **Owner-only** — extension is NOT permissionless. Step 1 pays the elapsed
    /// portion out to the provider, so if a third party (or the provider itself)
    /// could extend, it could force-settle the elapsed term and defer expiry
    /// indefinitely, stripping the owner of its burn/exit lever
    /// (design doc "The Burn Option"). Only the
    /// owner may spend its own locked payment this way, so only the owner extends.
    /// The `expected_version` pin still applies (owner pays no more than it saw).
    ///
    /// For a virtual provider: rejected unless every member in the agreement's
    /// snapshot is still in the live set (otherwise the owner creates a
    /// replacement), and rejected if the re-snapshotted set would push the
    /// bucket's slot layout past `MaxPrimarySlots` (virtual-provider extension).
    ///
    /// Ending an agreement early, against a proven successor, is
    /// `create_replacement` plus activation ("Replacement agreements"), also
    /// owner-only. A third party keeps a frozen/public bucket alive by funding
    /// its own replica, never by touching the owner's agreement (design doc
    /// "Permissionless persistence").
    ///
    /// Also fails if:
    /// - The agreement has expired (`AgreementExpired`)
    /// - Duration below provider's min_duration or above max_duration
    /// - Provider has globally paused extensions (settings.accepting_extensions == false)
    /// - Provider has blocked extensions for this specific bucket (agreement.extensions_blocked == true)
    /// - `expected_version != provider.version` (`ProviderVersionMismatch`)
    ///
    /// The re-snapshot bumps the provider's `cur_until` to the new expiry, so the
    /// extended stretch is backed by whatever stake is current at extension time
    /// — consistent with "Changeable Stake".
    #[pallet::weight(...)]
    pub fn extend_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        additional_duration: BlockNumberFor<T>,
        expected_version: u32,
    ) -> DispatchResult;

    /// Transfer agreement ownership (current owner only).
    /// 
    /// The new owner can top up quota and transfer ownership further.
    /// Useful for selling agreement slots or transferring to a DAO.
    ///
    /// **The escrow moves with the agreement.** The prepaid fee, and a
    /// replica's unspent sync balance, are held on the owner, so the hold
    /// moves to `new_owner` and stays a hold. Every later settlement and
    /// refund then uses the new owner.
    ///
    /// **Bucket membership does not move.** For a primary agreement the owner
    /// is a bucket admin at creation, and a transfer is the one way the two
    /// come apart: the new owner is not a member and cannot write to the
    /// bucket or administer it.
    ///
    /// **Challenge rights follow the owner.** The new owner joins the
    /// bucket's authorized challengers, and for a primary agreement on a
    /// private bucket may challenge primaries without being a member. Open
    /// challenges keep the tier they were created with.
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
    /// **After expiry:** Owner can call within T::SettlementTimeout (48h) to
    /// settle. If owner doesn't act, provider can call claim_expired_agreement
    /// (silence defaults to pay). 48h gives an owner room to act; and client
    /// software can automate it — e.g. the user marks "burn on end" mid-agreement
    /// and the client submits `end_agreement { Burn }` automatically once the
    /// agreement expires.
    ///
    /// **Why burn only at end, never mid-agreement:** burning zeroes the
    /// provider's payment, so for any time still left on the agreement it has no
    /// incentive left to *serve*. Storage/availability is still enforced (the
    /// slashing threat forces it to answer challenges), but retrievability is
    /// not: a burned-but-still-active provider becomes a zombie that stores the
    /// data yet stops serving reads off-chain, doing only the bare minimum to
    /// avoid a slash — e.g. waiting to be challenged and betting the challenge is
    /// cancelled once the reader gives up. That is worst exactly where we wanted
    /// protection: public challengers get no cost-split, so it can stonewall them
    /// at no cost to itself. Deciding burn only once the term is over avoids
    /// creating such a zombie during a still-live availability guarantee. (Same
    /// reason to be wary of early-termination-with-burn unless it also *ends* the
    /// agreement's obligations.)
    /// 
    /// **Should we have early termination for primaries?**
    /// Admin could use ability to remove hostile or misbehaving primary
    /// providers. Without this, a malicious primary could hold the bucket
    /// hostage until expiry. Primary providers are admin-controlled for write
    /// coordination; admin must maintain control over who can accept writes. I
    /// think we can avoid this, by just having the number of allowed providers
    /// high enough, to make this scenario highly unlikely. Alternatively, we
    /// could enable early termination for primaries, but it should be
    /// exceptional: Burn not pay & only if at capacity for example.
    /// 
    /// **Replicas cannot be early-terminated:** There's no use case, and allowing
    /// it would violate the principle of least surprise. A business checking on a
    /// bucket sees "5 providers with agreements until May" and concludes all is
    /// well - they shouldn't find the bucket dead the next day because someone
    /// terminated agreements early. If unhappy with a provider, simply don't extend.
    /// 
    /// Note: For primary agreements, admin is the owner at creation (via
    /// `create_bucket_with_primary` / `add_primary_provider`);
    /// `transfer_agreement_ownership` can separate them.
    /// Admin has no special privileges over replica agreements.
    ///
    /// Blocked while a challenge against `(bucket, provider)` is unresolved
    /// (`AgreementHasPendingChallenge`, via `PendingChallengesByBucket`): an
    /// agreement cannot be settled out from under a live slashable challenge.
    /// The same guard applies to `claim_expired_agreement` below.
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
    /// Blocked while a challenge against `(bucket, provider)` is unresolved
    /// (`AgreementHasPendingChallenge`): the provider must not claim and
    /// exit while still slashable.
    #[pallet::weight(...)]
    pub fn claim_expired_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
    ) -> DispatchResult;

    /// Give the agreement's remaining payment back to its owner (**provider
    /// only**).
    ///
    /// Origin: the provider, with `approvals` empty; for a virtual provider,
    /// any live member, with `approvals` carrying member signatures over the
    /// refund and the virtual's `governance_nonce` that together hold a
    /// seniority majority (virtual-provider extension, "Payment" and
    /// "Membership Governance").
    ///
    /// For a provider that finds it cannot serve the agreement adequately.
    /// Returns the whole remaining `payment_locked` to the owner. The agreement
    /// stays live and unpaid until expiry: still challengeable,
    /// `committed_bytes` unchanged, the provider liable for everything it
    /// already committed to. At settlement there is nothing to pay or burn, and
    /// the record is removed as usual. A pending replacement keeps its own
    /// payment. Increments `agreements_refunded`; emits `AgreementRefunded`.
    #[pallet::weight(...)]
    pub fn refund_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        approvals: BoundedVec<(T::AccountId, Signature), T::MaxPhysicalMembers>,
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
    /// Requires at least `min_providers` signing slots of the bucket's layout
    /// (`BucketSnapshot.primary_signers`). A virtual primary's signatures in one
    /// call are all-or-nothing: at least its threshold `k`, or none — fewer
    /// rejects the call (virtual-provider extension, "Checkpoints"). Signatures of a pending
    /// replacement's provider set are accepted only to activate it
    /// ("Replacement agreements").
    /// For frozen buckets: start_seq must equal frozen_start_seq (only leaf_count can increase).
    pub fn checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        commitment: Commitment,
        signatures: BoundedVec<(T::AccountId, Signature), T::MaxPrimarySlots>,
    ) -> DispatchResult;

    /// Extend an existing checkpoint's provider bitfield (anyone can call).
    /// 
    /// Adds additional provider signatures to the current snapshot without changing
    /// the mmr_root, start_seq, or leaf_count. This is permissionless because:
    /// - It only adds accountability (more providers are now challengeable)
    /// - It cannot change the canonical state
    /// - Signatures are verified on-chain
    /// 
    /// Providers added this way become liable for the snapshot state. For a
    /// virtual primary, either its whole group at ≥`k` in one call, or further
    /// members of a group that already reached `k` in this snapshot
    /// (virtual-provider extension, "Checkpoints").
    pub fn extend_checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        additional_signatures: BoundedVec<(T::AccountId, Signature), T::MaxPrimarySlots>,
    ) -> DispatchResult;

    /// Add members to an open off-chain challenge against a virtual provider
    /// (anyone can call).
    ///
    /// Verifies further signatures over the challenged `CommitmentPayload` from
    /// members of the challenged agreement's snapshot and adds them to the
    /// challenge's liable set. Fails for a physical provider. Permissionless
    /// like `extend_checkpoint`: it only adds accountability. Without it a
    /// challenger could present the minimum `k` signatures and let the other
    /// signers walk away (virtual-provider extension, "Stake and Slashing").
    pub fn extend_challenge(
        origin: OriginFor<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        additional_signatures: BoundedVec<(T::AccountId, Signature), T::MaxPhysicalMembers>,
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
    // - Uses the replica's on-chain sync confirmation (the `last_sync` root;
    //   for a virtual provider, `last_sync_signers` are the ones slashed)
    // - No signature needed - chain already has their commitment
    // - Replicas are liable for roots they've confirmed synced to
    //
    // For hot buckets, challenge_checkpoint may fail due to race conditions,
    // but this is acceptable: active writers have signatures and can use
    // challenge_offchain. The snapshot primarily protects cold/archival data
    // where nobody has recent signatures or doesn't bother to dig them up.
    //
    // **Who may challenge, and at what cost (all three modes):**
    // Any signed account may challenge, except the challenged provider
    // itself (`SelfChallenge`: a free response that would pad its own
    // defended counters), with one restriction: on a `Private`
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
    /// Provider must be in current snapshot's provider list **and** have a live
    /// agreement for this bucket (`AgreementNotFound` otherwise). Liability
    /// tracks the agreement, not lingering snapshot membership: once a
    /// provider's last agreement ends it is un-challengeable — even if it is
    /// still named in an un-superseded snapshot — so it can deregister and
    /// withdraw stake immediately with no post-expiry race.
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

    /// Challenge off-chain commitment (requires the provider's signatures).
    /// Works regardless of current snapshot state - the signatures prove
    /// the provider committed to this data. `provider_signatures` must satisfy
    /// the provider signature rule (`ProviderSignatures`) over the
    /// `CommitmentPayload`; for a virtual provider the member set is the
    /// agreement's snapshot, and the accepted signers form the challenge's
    /// liable set (`extend_challenge` adds more).
    /// On a `Private` bucket, the gate applies iff the challenged provider's
    /// current agreement has role `Primary`
    /// (`NotAuthorizedForPrivateBucket`; role-based gate, see above).
    /// Rejected if `agreement_id` is not the provider's live agreement for this
    /// bucket (`AgreementNotFound`) — a commitment is void once its agreement
    /// ends, so obsolete signatures can't be used to slash.
    /// 
    /// Preferred for hot buckets where snapshots change frequently.
    pub fn challenge_offchain(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        agreement_id: u64,
        commitment: Commitment,
        target: ChunkLocation,
        provider_signatures: ProviderSignatures<T>,
    ) -> DispatchResult;

    /// Challenge a replica based on their on-chain sync confirmation.
    /// Uses the replica's `last_sync` root stored in their agreement.
    /// No signature needed - the chain already has their commitment. For a
    /// virtual provider, a failed challenge slashes the members in
    /// `last_sync_signers`.
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
    /// Called by the replica provider named in `provider`; for a virtual
    /// provider, by any member of the replica agreement's snapshot.
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
    /// - `provider`: The replica provider (for a virtual, its synthetic account)
    /// - `roots`: Array of optional MMR roots [current, pos0, pos1, pos2, pos3, pos4, pos5].
    ///   Replica sets Some(root) for positions they have, None for positions they don't.
    /// - `signatures`: the provider's signatures over the roots array,
    ///   satisfying the provider signature rule (`ProviderSignatures`). For a
    ///   virtual provider the member set is the agreement's snapshot; the
    ///   accepted signers are stored in `last_sync_signers` and are the ones
    ///   slashed by a failed `challenge_replica` against it.
    #[pallet::weight(...)]
    pub fn confirm_replica_sync(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        /// Array of optional MMR roots: [current, pos0, pos1, pos2, pos3, pos4, pos5]
        /// The provider signs this to attest which roots it has.
        roots: [Option<H256>; 7],
        signatures: ProviderSignatures<T>,
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

    /// Cancel an active challenge.
    ///
    /// Allows the challenger to cancel if they received the data off-chain.
    /// Full deposit is refunded (only transaction fees are lost). This prevents
    /// unnecessary on-chain data submission when the issue was resolved
    /// off-chain. Can only be called by the original challenger.
    #[pallet::weight(...)]
    pub fn cancel_challenge(
        origin: OriginFor<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
    ) -> DispatchResult;
}

pub enum EndAction {
    /// Pay provider in full
    Pay,
    /// Burn locked payment entirely.
    /// Additionally deducts `T::BurnPremium` (e.g., 10%) from caller's free balance.
    /// Fails if caller has insufficient funds for the premium.
    Burn,
}

pub enum RemovalReason {
    /// Provider was slashed for failing a challenge
    Slashed,
    /// Admin terminated agreement early - iff we end up wanting this - see previous comment.
    AdminTerminated,
    /// Agreement expired naturally
    Expired,
}

/// Why a provider was slashed; reported in `ChallengeSlashed` and returned by
/// `verify_challenge_response` (see "Verification").
pub enum SlashReason {
    /// No response before the challenge deadline
    Timeout,
    /// `Proof` response whose chunk or MMR proof did not verify
    InvalidProof,
    /// `Deleted` response whose `new_start_seq` does not cover the challenged
    /// leaf or whose admin signature does not verify
    InvalidDeletionClaim,
    /// `Superseded` response without a canonical snapshot that replaces the
    /// challenged root and covers the challenged leaf
    InvalidSupersededClaim,
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
        admin: T::AccountId,
        admin_signature: Signature,
    },
    /// Challenged state has been superseded by a larger canonical checkpoint.
    /// Valid when: canonical.mmr_root != challenged mmr_root AND
    /// canonical.start_seq <= challenged_seq < canonical.start_seq + canonical.leaf_count
    /// (The leaf exists in canonical - challenger should challenge the snapshot instead)
    /// (If the challenged root IS the canonical root, the data is live - must use Proof)
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

Every endpoint that returns a provider signature returns `provider_signatures`,
a list of `{ "signer": "<ss58>", "signature": "0x..." }`: one entry from a
physical provider; from a member of a virtual provider, at least `k` entries,
its own and those it collected from the other members (the write path's
collection round), ready to pass on-chain as `ProviderSignatures`.

### Authentication & RBAC

Mutating Layer-0 endpoints (`PUT /node`, `POST /commit`, `POST /delete`) and
authenticated read endpoints require an `Authorization` header. The provider node verifies an sr25519 signature
locally and resolves the caller's role via a TTL-cached query against the
chain's `Buckets` storage (`bucket.members`).

> **⚠️ Under-specified — [#304](https://github.com/paritytech/web3-storage/issues/304).**
> This scheme grew organically across several crates and needs one source of
> truth: the wire format is currently defined twice (Rust `provider-auth`
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
- Membership is cached: a chain event invalidates the affected bucket
  immediately; missing that, `--auth-cache-ttl` (default 30s) bounds the
  delay. If a refetch then fails, the cached set is served for up to
  `--auth-max-stale` (default 5 minutes) before the request is refused with
  `503 membership_unavailable`.
- A member whose `Role` the provider cannot decode is not authorized — the
  lookup fails rather than falling back to a lesser role.

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
  "agreement_id": 7,                            // agreement the commitment is under
  "data_roots": ["0xroot1...", "0xroot2..."]   // roots to add to MMR
}

Response (200 OK):
{
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 7,  // number of leaves after the commit
  "leaf_indices": [5, 6],  // indices assigned to each data_root
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
    // over CommitmentPayload{ version, bucket_id, agreement_id, commitment }
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

Negotiate Terms
───────────────
POST /negotiate

Request:
{
  "owner": "<ss58 account that redeems the quote>",
  "bucket": 1234 | null,
  "max_bytes": "1073741824",
  "duration": 201600,
  "nonce": 3,   // the owner's `AgreementNonces` value this quote will consume
  "replica_params": null | { "sync_balance": 5000000000, "min_sync_interval": 0 }
}

`bucket` is the bucket id the quote is for, or `null` for a bucket created at
redemption; the node maps it to `BucketTarget`. The node signs the terms as
requested, adding only `valid_until`.

The quote carries no price and no version. When redeeming, the client passes
the version at which it read the provider's terms as `expected_version`; the
chain reads price, replica sync price and stake from the provider and fails with
`ProviderVersionMismatch` if that version has moved (see "Term Pinning").

Response (200 OK):
{
  "provider": "<ss58 of the provider the quote is for; a virtual's synthetic account>",
  "terms": { ...AgreementTerms as signed },
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
}

To pass to `create_bucket_with_primary` (`bucket: null`),
`add_primary_provider` (`bucket` set, `replica_params: null`) or
`add_replica_provider` (both set), with `provider` as the call's `provider`
and `provider_signatures` as its `sigs`. A member of a virtual provider collects at least `k` member signatures
over the terms before responding; if it cannot, it rejects the request.

The provider rejects requests outside its duration bounds, beyond its capacity,
or for a role it is not accepting.

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
GET /commitment?bucket_id=1234&agreement_id=7

Response:
{
  "bucket_id": 1234,
  "agreement_id": 7,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
}

Note: The returned signatures cover a `CommitmentPayload` with the real
`leaf_count`; `challenge_offchain` reconstructs the payload from the
`commitment` and `agreement_id` the challenger passes, so the same values
returned here must be passed on-chain unchanged.

Get Checkpoint Signature (for checkpoint extrinsic)
───────────────────────────────────────────────────
GET /checkpoint-signature?bucket_id=1234&agreement_id=7

Response:
{
  "bucket_id": 1234,
  "agreement_id": 7,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
}

Note: Signs the same payload as `/commitment`; kept as a separate endpoint
for the checkpoint workflow, where the signatures go into the
`checkpoint`/`extend_checkpoint` signature list (a virtual provider's entries
all-or-nothing, see `checkpoint`).

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
  "agreement_id": 7,
  "new_start_seq": 10
}

Response (200 OK):
{
  "mmr_root": "0xnew...",
  "start_seq": 10,
  "leaf_count": 5,
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
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
   a. Sign the roots array matching on-chain historical_roots (a virtual
      provider's members collect ≥`k` signatures between their nodes, as for
      commitments)
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
group inside `CommitmentPayload` and `BucketSnapshot`, and the single argument
the checkpoint/challenge extrinsics take in place of three loose fields.

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
    /// Protocol version for future compatibility. CURRENT_VERSION = 1 on
    /// `dev` today; adding `agreement_id` below is a layout change and bumps
    /// it to 2 so v1 signatures cannot verify against the new payload.
    pub version: u8,
    /// Reference to on-chain bucket. Mandatory — there is no anonymous /
    /// "best-effort" commitment mode in the current implementation.
    pub bucket_id: BucketId,
    /// Agreement this commitment is made under. A challenge validates the
    /// commitment only while that agreement is live; once it ends the
    /// commitment is void (see `StorageAgreement.agreement_id`). This is the
    /// sole replay bound: a signature is usable only against the exact agreement
    /// it names, and dies with that agreement — no separate time-based nonce is
    /// needed (see "Replay & commitment validity").
    pub agreement_id: u64,
    /// MMR commitment being signed over
    pub commitment: Commitment,
}
```

> **Cross-chain uniqueness (future).** `bucket_id` is unique only *per chain*, so
> if this pallet ever runs on more than one parachain, a commitment signed on
> chain A could be replayed against the same `bucket_id` on chain B. When that
> happens, add the **`para_id`** to `CommitmentPayload` (and the checkpoint
> payload) to domain-separate signatures per chain. Not needed while a single
> chain exists; an absent/zero `para_id` defaults to the base para, so the field
> can be introduced later without breaking existing single-chain signatures
> (behind the `version` bump).

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
    /// The leaf being proven. Verification hashes `leaf.encode()` as the
    /// proof's starting point, so the leaf content is part of the proof.
    pub leaf: MmrLeaf,
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
/// Judged in one step. A valid response settles the deposit; an invalid
/// one slashes the provider on the spot with the returned reason. Only a
/// malformed submission — unknown challenge, wrong provider, past the
/// deadline, or a `Deleted` claim naming a signer who is not a bucket
/// admin — fails as a plain dispatch error the provider may correct and
/// resend.
fn verify_challenge_response(
    challenge: &Challenge,
    response: &ChallengeResponse,
    bucket: &Bucket,
) -> Result<(), SlashReason> {
    let challenged_seq = challenge.start_seq + challenge.target.leaf_index;
    match response {
        ChallengeResponse::Proof { chunk_data, mmr_proof, chunk_proof } => {
            // The chunk must sit in the leaf and the leaf in the committed MMR.
            let chunk_hash = blake2_256(chunk_data);
            let chunk_ok = verify_merkle_proof(
                chunk_hash, challenge.target.chunk_index, chunk_proof, &mmr_proof.leaf.data_root,
            );
            let mmr_ok = verify_mmr_proof(mmr_proof, &challenge.mmr_root);
            if chunk_ok && mmr_ok { Ok(()) } else { Err(SlashReason::InvalidProof) }
        }

        ChallengeResponse::Deleted { new_mmr_root, new_start_seq, admin, admin_signature } => {
            // Note: We don't check frozen_start_seq here. Freeze protects canonical
            // checkpoints (enforced at checkpoint time), but off-chain deletions can
            // race with freeze. If admin signed a deletion, provider has valid defense
            // regardless of freeze state. Off-chain is "messy but functional."
            //
            // `admin` must be a bucket admin (dispatch error otherwise, see above).

            // The purge must actually cover the challenged leaf.
            if challenged_seq >= *new_start_seq {
                return Err(SlashReason::InvalidDeletionClaim);
            }
            // And the admin must have signed the newer commitment.
            let payload = CommitmentPayload::new(
                challenge.bucket_id,
                Commitment { mmr_root: *new_mmr_root, start_seq: *new_start_seq, leaf_count: 0 },
            );
            if verify_signature(admin_signature, &payload.encode(), admin) {
                Ok(())
            } else {
                Err(SlashReason::InvalidDeletionClaim)
            }
        }

        ChallengeResponse::Superseded => {
            // Provider can defend if the challenged commitment was replaced by a
            // newer canonical snapshot that still covers the challenged leaf:
            // same data re-committed, or a forked branch that lost. No admin
            // signature is needed — canonical may have evolved without this
            // provider.
            //
            // Deleted vs Superseded:
            // - Deleted: requires admin signature, works without canonical snapshot,
            //   covers data purged from the front (challenged_seq < new_start_seq)
            // - Superseded: requires canonical snapshot, works without admin signature,
            //   covers data still inside the canonical range
            // Data rolled off the front of canonical is NOT a Superseded defense;
            // it must go through the admin-signed Deleted path.
            //
            // Provider IS liable when the challenged root is still canonical (the
            // data is live, only a Proof defends it) or when challenged_seq lies
            // beyond canonical_end (they signed something canonical never covered).
            let Some(snapshot) = bucket.snapshot.as_ref() else {
                // Nothing canonical to lean on: the claim is unsupported.
                return Err(SlashReason::InvalidSupersededClaim);
            };
            if challenge.mmr_root != snapshot.commitment.mmr_root
                && snapshot.contains_seq(challenged_seq)
            {
                Ok(())
            } else {
                Err(SlashReason::InvalidSupersededClaim)
            }
        }
    }
}
```

### Response transaction extension

A response carries a chunk of up to 256 KiB, so its fee is dominated by length.
Two responses to one challenge — a provider racing its challenger's
`cancel_challenge`, or two members of a virtual provider — would today both be
included and both charged: transaction validity does not consult pallet state,
and a dispatch that fails still pays. A runtime `TransactionExtension` (the
`CheckNonce` pattern) handles `respond_to_challenge { challenge_id, .. }`:

- `validate` fails with `Stale` if the challenge does not exist and with
  `BadSigner` if the signer is not an eligible responder — the challenged
  provider, or for a virtual provider a member of the challenged agreement's
  snapshot. Such a transaction is rejected by the first pool that sees it and
  never gossiped further.
- Otherwise it returns `provides: [challenge_id]`. The pool keeps at most one
  ready transaction per tag, so a second response is rejected at import. Once a
  response is included, every pool extracts its tags (validating at the parent
  block, where it is still valid) and prunes the others. A straggler that
  reaches the block builder anyway fails `validate` there and is dropped
  uncharged.

The loser of a race pays nothing and takes no block space. A response that
passes `validate` but fails at dispatch — a squatter taking the tag with garbage
— is included and pays the full fee. Other calls pass through the extension
unchanged.

---

## Open Questions
