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
`Private` bucket, only members may challenge a primary (replicas stay
challengeable by anyone).

**Redundancy**: A bucket can have storage agreements with multiple providers. The `min_providers` setting is the number of slots in the bucket's slot layout that must sign a state before it can be checkpointed. A physical primary fills one slot. A virtual primary fills one slot per member (see `BucketSnapshot.primary_signers`). This ensures minimum redundancy for critical data.

**Append-only mode**: When `frozen_start_seq` is set, the bucket becomes append-only from that point. The start_seq stays at the frozen value, preventing deletion of historical data. This is irreversible and requires the current snapshot to still meet `min_providers`. `freeze_bucket` counts the snapshot's set bits again, only in slots of live agreements: primaries may have left or expired, or `min_providers` may have risen since the checkpoint. At least that many liable providers then hold the state the admin can no longer delete. Admins can delete a frozen bucket only once it has no agreements left (one `StorageAgreements` prefix read).

### Storage Model

**Upload and Commit are separate operations**:

1. **Upload**: Clients upload content-addressed data (chunks and internal nodes) to providers. This is just storage — no MMR involvement yet. Providers accept every canonical chunk-tree node (see "Content-Addressed Storage" in the provider API) as long as the bucket has quota. Multiple clients can upload different data concurrently without conflicts.

2. **Commit**: A client names a base commitment of the bucket's MMR and requests the provider to append data_root(s) to it. The provider signs the resulting commitment. The client computes that commitment itself (see "Commit"). This is when data becomes "committed" and the provider becomes liable.

3. **Checkpoint**: A client submits provider signatures to the chain, establishing canonical state. The chain records which providers acknowledged this state. Only providers in the snapshot are challengeable for this state.

**No conflict rejection**: Providers store every node that passes the `PUT /node` acceptance rules, within quota. "Conflicts" (different clients uploading different data) are fine — the checkpoint determines which state becomes canonical.

**Node pruning rule**: A provider may delete a stored chunk or internal node only if both conditions hold:

- no leaf the provider retains has the node as its `data_root`;
- no other stored node has the node as a child.

After the provider deletes an internal node, it applies the same rule to the node's two children. For uploaded data that no commitment references, the provider deletes roots oldest first, when the bucket's quota is exhausted or after a timeout the provider chooses. A reference count per node is sufficient to implement the rule.

**Pruning rule**: A primary may prune a leaf of a signed commitment only if all of these hold:

- the commitment is not the current snapshot's commitment;
- the leaf is below the snapshot's end (`start_seq + leaf_count`);
- the leaf is below the snapshot's `start_seq`, or the snapshot's leaf at the same sequence number is a different `MmrLeaf`.

A pruned leaf no longer counts as a reference under the node pruning rule. The primary answers a challenge on a pruned leaf with `Deleted` if the leaf is below `start_seq`, otherwise with `Superseded`.

Every leaf the current snapshot contains stays. `challenge_checkpoint` enforces this for a primary with a set bit in the snapshot's `primary_signers`. For any other primary, keeping these leaves is a service duty: the primary needs them to sign a later checkpoint or `extend_checkpoint`.

Pruning is permanent. A checkpoint only appends to the canonical MMR or starts a new MMR at its end (`checkpoint`), so a pruned leaf never becomes canonical.

**Optional snapshots**: On-chain snapshots are optional. Without a snapshot:
- `challenge_offchain` works (challenger provides provider signature)
- `challenge_checkpoint` fails (nothing to challenge)
- `Superseded` defense unavailable (no canonical to compare against)
- `Deleted` defense unavailable (nothing can be deleted)
- Provider is liable for ALL signed commitments
- Conflicting forks cannot be pruned

Users who create conflicts without checkpointing waste their quota—providers must keep all signed data.

**Content-addressed storage**: Everything (chunks and internal nodes) is addressed by hash. An internal node is its two child hashes. Chunks and nodes are hashed with different prefixes (`0x00` for a chunk, `0x01` for a chunk-tree node). Upload is bottom-up: children must exist before parent can be stored. The provider accepts a node only if the node is the root of an RFC 9162 tree over `CHUNK_SIZE` chunks (see "Chunk Tree"). The provider rejects other shapes and chunk sizes at upload ("Content-Addressed Storage" in the provider API). Together with the node pruning rule, this means every stored root has its entire canonical tree stored. A commit therefore only checks that the root exists.

### Provider Lifecycle in Bucket

Bucket creation and provider assignment are separate on-chain operations.
Four calls cover them:

| Call | Who | Effect |
| --- | --- | --- |
| `create_bucket` | anyone | Empty bucket, caller is sole admin and pays its storage deposit |
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
4. Client requests commit, provider signs → client verifies the signature against the commitment it computed (see "Commit")
5. Client calls `checkpoint` with this provider's signature and those of the other primaries → the checkpoint's signers replace `snapshot.primary_signers`

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
1. After `SettlementTimeout`, anyone may call `claim_expired_agreement`, which pays the provider (for a virtual, its snapshotted members), OR
2. Client calls `end_agreement` with pay/burn decision within settlement window
3. Provider is no longer bound to store data, except while a challenge it must answer is open (see "Off-Chain: Provider Node API")
4. Provider won't be included in future checkpoints

**Bucket without providers:** ending the last agreement removes the provider
from `bucket.primary_providers` and deletes the agreement, not the bucket.
The bucket, its members, `snapshot` and `frozen_start_seq` remain on-chain.
The admin can add a new primary with `add_primary_provider` at any later
time and re-upload the data; the `bucket_id` and every external reference to
it stay valid. The bucket remains until an admin calls `delete_bucket`.

**Commitment liability**: A provider is liable for every commitment it signed, checkpointed or off-chain, while the commitment's agreement is live, unless the `Deleted` or `Superseded` defense applies (`verify_challenge_response`). For the leaves the current snapshot covers, the snapshot voids a primary's other commitments. Redundancy for those leaves comes from `min_providers`. A replica is liable in the same way for its last confirmed sync (`confirm_replica_sync`). That sync is always a canonical state (`checkpoint`): a prefix of the current snapshot, whose leaves the pruning rule never removes, or below the snapshot's `start_seq`. A replica keeps its last confirmed sync until its next sync, also after a deletion, because `Superseded` and `Deleted` are not defenses for it. Otherwise providers prune data only as the pruning rule or a deletion allows.

### Multi-Provider Coordination (Primary Providers)

Primary providers don't sync with each other. Clients are responsible for uploading to each primary provider they want to store their data.

**Flow**:
1. Client uploads data to Primary A, B, C (separately)
2. Client commits on each provider so that every provider reaches the same resulting commitment. The client computes that commitment itself and verifies each provider's signatures against it (writer steps in "Commit")
3. Client checkpoints on-chain with its own commitment and the collected signatures. For an append to a non-empty snapshot, the client also submits one primary's consistency proof (`/checkpoint-signature`)
4. Primaries not in the snapshot should sync (client re-uploads)
5. After checkpoint, providers can prune non-canonical roots

**The client moves the data**: a primary added to a bucket that already has
data receives that data from the client, not from the chain and not from the
other providers. The client uploads the data to the new primary, commits it
from the empty MMR at the snapshot's `start_seq` so the new primary builds
the full MMR ("Commit", step 2), and includes
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

    /// Receives slashed stake. The runtime decides the destination; on a
    /// Polkadot system parachain the convention is
    /// `pallet_accumulate_and_forward`, which forwards to the DAP on Asset Hub.
    type OnSlash: OnUnbalanced<Credit<Self::AccountId, Self::Currency>>;

    /// Receives burned payments and the `BurnPremium`. Same destination
    /// convention as `OnSlash`.
    type OnBurn: OnUnbalanced<Credit<Self::AccountId, Self::Currency>>;

    /// Premium the owner pays from free balance, on top of the locked
    /// payment, when it burns an agreement (design doc "The Burn Option").
    #[pallet::constant]
    type BurnPremium: Get<Permill>;

    /// Maximum length of provider multiaddr.
    #[pallet::constant]
    type MaxMultiaddrLength: Get<u32>;

    /// Maximum members per bucket (`Bucket.member_count`).
    #[pallet::constant]
    type MaxMembers: Get<u32>;

    /// Maximum signing slots per bucket across its primary agreements. A
    /// virtual primary counts as one slot per member. Bounds the checkpoint
    /// bitfield and the signatures verified per checkpoint. The calls that
    /// add slots (`add_primary_provider`, `create_replacement`) count a
    /// primary with a pending successor at the larger of its current slots
    /// and the successor's members, so activating a successor never exceeds
    /// this bound. Must be at most 128: a `checkpoint` slot index is a `u8`,
    /// and its index space (current and pending slots) is at most twice
    /// this bound.
    #[pallet::constant]
    type MaxPrimarySlots: Get<u32>;

    /// Maximum members of a virtual provider (virtual-provider extension);
    /// `<= min(MaxPrimarySlots, 8)`: `VirtualLiability.liable` and
    /// `last_sync_signers` are `u8` bitmasks.
    #[pallet::constant]
    type MaxPhysicalMembers: Get<u32>;

    /// Minimum stake required to register as a provider.
    /// Governance-controlled to bound total provider count and provide sybil resistance.
    #[pallet::constant]
    type MinProviderStake: Get<BalanceOf<Self>>;

    /// Upper bound of `chunk_data` in a challenge response: at least the
    /// `CHUNK_SIZE` of every leaf version the pallet accepts. A leaf's chunk
    /// count comes from its own version's `CHUNK_SIZE`, not from this value,
    /// so a runtime upgrade that changes it does not change `n` for any leaf.
    #[pallet::constant]
    type MaxChunkSize: Get<u32>;

    /// Deposit held from the challenger when a challenge is created. Sized to
    /// the expected fee of a full `Proof` response plus a margin. A challenge
    /// is rejected if the deposit cannot be held.
    #[pallet::constant]
    type ChallengeDeposit: Get<BalanceOf<Self>>;

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

    /// Storage deposit for records a user creates: provider registrations,
    /// buckets, member entries and agreements. Each record stores its ticket,
    /// priced from a fixed `Footprint` per record kind, so a ticket is never
    /// updated while its record exists. The runtime uses a
    /// `HoldConsideration` under `HoldReason::StorageDeposit`: the deposit
    /// stays on hold on the depositor until the pallet drops the ticket with
    /// its record.
    type StorageDeposit: Consideration<Self::AccountId, Footprint>;

    /// Caps the challenges sharing one deadline (anchor block); bounds the
    /// per-deadline index allocator.
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
| `MaxMembers` | `1000` |
| `MaxPrimarySlots` | `8` |
| `MaxPhysicalMembers` | `4` |
| `MaxChunkSize` | `262_144` (256 KiB, the `CHUNK_SIZE` of leaf version 1) |
| `ChallengeDeposit` | `10 * UNIT` // TODO: size from the benchmarked respond_to_challenge weight |
| `ChallengeTimeout` | `48 * RC_HOURS` |
| `SettlementTimeout` | `48 * RC_HOURS` |
| `RequestTimeout` | `6 * RC_HOURS` |
| `StorageDeposit` | `HoldConsideration<AccountId, Balances, StorageDepositReason, LinearStoragePrice<StorageDepositBase, StorageDepositPerByte, Balance>>` with base `UNIT / 10` and `MICROUNIT` per byte, the rates the contracts pallet (`revive.rs`) charges per regular storage item and per byte |
| `MaxChallengesPerDeadline` | `1_000` |
| `AnchorBlockTimeMillis` | `6_000` |
| `OnSlash`, `OnBurn` | `ResolveTo` the account derived from `PalletId(*b"py/trsry")` |
| `BurnPremium` | `Permill::from_percent(10)` |

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
    /// A deposit for a record a user created, held on the depositor until
    /// the record is removed.
    StorageDeposit,
}
```

An agreement's escrow always sits on its **owner** (permissionless top-ups move
a third party's funds there first, since settlement pays out of the owner's
hold), and — unlike `reserve` — a hold must leave the existential deposit
spendable, so registering needs `stake + storage deposit + ED` of free balance
and an account with a hold cannot be reaped.

Each user-created record also has a **storage deposit**, held on the
provider (provider record), the bucket creator (bucket), the admin that added
the member (member entry) or the account that opened the agreement
(agreement). Removing the record releases it to that account. Genesis
providers and buckets pay it from the genesis endowment.

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
    /// Grow-only max expiry over agreements struck at the current `stake`
    /// generation. Bumped by agreement creation, `extend_agreement` and
    /// `create_replacement`; never decremented. Folded
    /// into `higher_stake_lock` when `stake` is lowered, then reset to zero.
    pub cur_until: BlockNumberFor<T>,
    /// A previous, higher stake generation still owed to live agreements.
    /// Blocks a further lowering while `lock_active(until, p)` ("Changeable Stake").
    /// Set only by `set_stake` when lowering.
    pub higher_stake_lock: StakeLock<BalanceOf<T>, BlockNumberFor<T>>,
    /// Open challenges against this provider (for a virtual provider: on its
    /// synthetic account). While > 0 the provider keeps its locks and cannot
    /// withdraw stake ("Changeable Stake"); the members of a challenged
    /// virtual provider have their stake locked through it (virtual-provider
    /// extension).
    pub pending_challenges: u32,
    /// Total contracted bytes (sum of max_bytes across all agreements).
    /// Verifiable, unlike the self-declared `max_capacity`; informative only,
    /// no stake check is tied to it (see "Stake vs. capacity").
    pub committed_bytes: u64,
    /// Pins the provider-side terms an agreement is struck against. Every call
    /// that binds an owner to the provider's terms — the three redemption calls,
    /// `extend_agreement`, `top_up_agreement` and `create_replacement` — takes the client's
    /// `expected_version` and fails if it changed. This is what removes the
    /// need for a `max_payment` bound. Bumped on the worse-direction change to a term the
    /// quote/request doesn't carry explicitly: **price ↑**, **replica sync price
    /// ↑**, **stake ↓**, or (virtual providers) a member leaving or `k` rising. Duration,
    /// capacity and `accepting_*` are checked directly against the quote's
    /// `max_bytes`/`duration`, and strictly-better changes never bump. See
    /// "Term Pinning".
    pub version: u32,
    /// Block of the last slash that may have reduced the backing behind this
    /// provider's agreements: a slash of the provider itself, a failed challenge against
    /// it (virtual provider), or a slash elsewhere that left one of its members
    /// with less stake than the member's `ProviderVirtuals` entry for this
    /// virtual owes. `None` if never bumped. Lets a client detect a slash from
    /// state instead of the event log
    /// (virtual-provider extension, "Stake and Slashing").
    pub last_stake_event: Option<BlockNumberFor<T>>,
    /// Provider settings
    pub settings: ProviderSettings<T>,
    /// Provider statistics - clients use these to evaluate quality
    pub stats: ProviderStats<T>,
    /// Storage deposit, held on the provider at registration beside the
    /// stake and released by `deregister_provider`.
    /// Footprint: the record's maximum encoded length.
    /// For a virtual provider: virtual-provider extension, "On-Chain Changes".
    pub deposit: T::StorageDeposit,
}

/// A higher, previous stake generation still owed to live agreements.
/// Default `{ stake: 0, until: 0 }` = none.
pub struct StakeLock<Balance, BlockNumber> {
    /// The higher stake figure that stays locked.
    pub stake: Balance,
    /// Block until which it is owed; lowering is blocked while
    /// `lock_active(until, p)` ("Changeable Stake").
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
    /// Challenges from authorized challengers (`Challenge.authorized`) that
    /// the provider responded to. Counted at resolution. Cancelled challenges
    /// are not counted.
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
    /// Minimum `max_bytes` per agreement, primary or replica. `0` means no
    /// minimum. Redeem calls reject `terms.max_bytes` below this value.
    pub min_bytes: u64,
}

/// Monotonically increasing bucket ID counter. Ensures stable, unique IDs.
#[pallet::storage]
pub type NextBucketId<T: Config> = StorageValue<_, BucketId, ValueQuery>;

/// Bucket ID is a stable, unique identifier (not an index into a collection).
/// Using u64 ensures IDs never get reused even if buckets are deleted.
/// `BucketId` and `AgreementId` encode exactly as their inner integer: SCALE
/// does this for a single-field struct, and `#[serde(transparent)]` keeps
/// them plain JSON numbers in the provider HTTP API.
pub struct BucketId(pub u64);

/// Agreement ID, unique per bucket (`Bucket.next_agreement_id`); the pair
/// `(BucketId, AgreementId)` identifies an agreement on this chain. `u32`: ids
/// are minted only at agreement creation and replacement, so a bucket cannot
/// exhaust it. The counter must never wrap — an id reused within a bucket
/// would revive that agreement's void commitments.
pub struct AgreementId(pub u32);

/// Buckets: containers for data with membership and storage agreements
#[pallet::storage]
pub type Buckets<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    BucketId,
    Bucket<T>,
>;

/// Bucket members: bucket → account → `Member`. Every membership check is a
/// point read of `(bucket_id, account)`. The `bucket_members` runtime API
/// lists a bucket's members by prefix iteration. No call iterates over them.
#[pallet::storage]
pub type BucketMembers<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, BucketId,
    Blake2_128Concat, T::AccountId,
    Member<T>,
>;

/// A bucket member. The member's account is the `BucketMembers` key.
pub struct Member<T: Config> {
    pub role: Role,
    /// The admin that added the member and paid `deposit`. Fixed for the
    /// lifetime of the entry.
    pub depositor: T::AccountId,
    /// Storage deposit, released to `depositor` when the member or the bucket
    /// is removed. Footprint: two items, the `BucketMembers` entry and the
    /// `MemberBuckets` entry. Each item counts at the maximum encoded length
    /// of its keys and value.
    pub deposit: T::StorageDeposit,
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
    /// `Private` restricts challenges against a primary to members. Full
    /// semantics: design doc, "Bucket Visibility & Access".
    Private,
}

pub struct Bucket<T: Config> {
    /// Number of `BucketMembers` entries for this bucket, at most
    /// `T::MaxMembers`.
    pub member_count: u32,
    /// Number of members with role `Admin`, at least 1.
    pub admin_count: u32,
    /// Next agreement id to assign, incremented on each agreement created.
    /// Makes every agreement in this bucket uniquely identifiable over time so a
    /// commitment binds to a specific agreement (see `StorageAgreement.agreement_id`).
    pub next_agreement_id: AgreementId,
    /// Read visibility (see `Visibility`). On-chain, only the challenge
    /// extrinsics read it.
    pub visibility: Visibility,
    /// If Some, bucket is append-only from this start_seq.
    /// Checkpoints with start_seq != frozen_start_seq are rejected (prevents deletions).
    pub frozen_start_seq: Option<u64>,
    /// Minimum signed slots required for a checkpoint, 1 to
    /// `T::MaxPrimarySlots`. While the layout has fewer slots of live
    /// agreements, no checkpoint is possible; an admin adds a primary or
    /// lowers the value.
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
    /// Each entry stores (quotient, commitment) where quotient = block_number / prime.
    /// On each checkpoint, if current_block / prime != stored quotient, the entry
    /// is updated with (new_quotient, current snapshot commitment). This means each
    /// position remembers the commitment from the last time its prime boundary was
    /// crossed. The whole commitment, not just the root: `confirm_replica_sync`
    /// checks that it advances over the replica's `last_sync`.
    /// 
    /// Primes ensure positions don't align, maximizing coverage. A slow replica
    /// can match against older positions; `position_matched` in events tracks this.
    pub historical_roots: [(u32, Commitment); 6],
    /// Total snapshots created for this bucket (for statistics)
    pub total_snapshots: u32,
    /// The account that created the bucket and paid `deposit`. Fixed at
    /// creation. A bucket can have several admins, so the bucket stores the
    /// depositor apart from admin control; the creator gets the deposit back
    /// even if it is no longer an admin or member.
    pub creator: T::AccountId,
    /// Storage deposit for the bucket record, held on `creator` and released
    /// when `delete_bucket` removes the bucket.
    /// The ticket contains only the amount, so the record stores the
    /// depositor beside it, as `pallet-preimage` does.
    ///
    /// Footprint: one item of the encoded size of a new `Bucket` with no
    /// primary providers and no snapshot, including this field. The footprint
    /// is the same for every bucket. The pallet sums it from the field types,
    /// so it follows changes to `Bucket`. Every member entry, the creator's
    /// included, and every agreement carry their own deposits
    /// (`Member.deposit`, `StorageAgreement.deposit`). The agreement deposit
    /// covers the primary slot that `create_bucket_with_primary` seeds.
    pub deposit: T::StorageDeposit,
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
    /// so a set bit means a liable signer while its slot's agreement is
    /// live. `min_providers` counts set bits.
    ///
    /// A physical account belongs to at most one primary of a bucket,
    /// counting each primary's slots and its pending successor's snapshot, so
    /// it fills at most one slot of the layout. `add_primary_provider`, and
    /// `create_replacement` on a primary agreement, reject an account that
    /// belongs to another primary (`DuplicateSigner`). The check passes once
    /// the other primary's agreement settles (`end_agreement` or
    /// `claim_expired_agreement`), once a replacement of that primary without
    /// the account activates, or once the account leaves the virtual provider
    /// the rejected call names.
    ///
    /// `BitVec<u8, Lsb0>` whose length always equals the number of slots in
    /// the layout (bounded by `T::MaxPrimarySlots`), so indices are stable
    /// within a checkpoint. `checkpoint` builds it from the submitted
    /// signatures over the call's final layout. If the layout
    /// changes between checkpoints (a primary added or removed), that
    /// extrinsic adjusts the bits in place. A slot added after the checkpoint
    /// gets its bit set when it signs via `extend_checkpoint`.
    pub primary_signers: BitVec<u8, Lsb0>,
}
// Canonical range is [start_seq, start_seq + leaf_count)
// A deletion starts a new MMR at start_seq = old start_seq + old leaf_count (`checkpoint`)

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
    pub agreement_id: AgreementId,
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
    /// liable at this figure until the agreement expires, independent of later
    /// stake changes (see "Changeable Stake").
    pub stake: BalanceOf<T>,
    /// Agreement expiration
    pub expires_at: BlockNumberFor<T>,
    /// Whether provider has blocked extensions for this specific agreement
    pub extensions_blocked: bool,
    /// Provider role for this bucket.
    pub role: ProviderRole<T>,
    /// Start of the current payment stretch: the agreement's creation, its
    /// last `extend_agreement`, or the activation of its replacement. Each of
    /// the last two pays the elapsed period pro rata from this block, then
    /// sets this field to `now`.
    pub started_at: BlockNumberFor<T>,
    /// The account that opened the agreement and paid `deposit`. Fixed at
    /// creation; `transfer_agreement_ownership` does not change it.
    pub depositor: T::AccountId,
    /// Storage deposit, held on `depositor` and released to it wherever the
    /// agreement is removed: `end_agreement`, `claim_expired_agreement`.
    /// Footprint: the record's maximum encoded length. Applies at any price,
    /// so a zero-price agreement still costs its depositor something to keep.
    pub deposit: T::StorageDeposit,
    /// Owner-created successor, not yet live ("Replacement agreements").
    pub pending_replacement: Option<PendingReplacement<T>>,
    /// `Some` for a virtual provider: the members liable for this agreement
    /// and their slice (virtual-provider extension). `None` for a physical one.
    pub virtual_snapshot: Option<VirtualSnapshot<T>>,
}

/// What a virtual provider's agreement holds liable (virtual-provider
/// extension): the members snapshotted when the agreement was struck, and the
/// slice each of them is slashed for. Only replacement activation replaces
/// the snapshot. An in-place extension (`extend_agreement`) keeps `members`
/// and re-snapshots only `per_provider_stake`.
///
/// The threshold is `stake / per_provider_stake`. The stake is stored next to
/// the snapshot: `StorageAgreement.stake`, or `PendingReplacement.stake` for a
/// pending successor. Every verification against the snapshot uses this
/// threshold, never the live `k`:
///   - the checkpoint all-or-nothing rule;
///   - `extend_checkpoint`;
///   - `challenge_offchain`;
///   - `confirm_replica_sync`;
///   - replacement activation.
pub struct VirtualSnapshot<T: Config> {
    /// Liable members, in checkpoint slot order.
    pub members: BoundedVec<T::AccountId, T::MaxPhysicalMembers>,
    /// `per_provider_stake` in force at creation or the last extension.
    pub per_provider_stake: BalanceOf<T>,
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
        /// 0 means no time-based limit (only the advancing-end check applies).
        min_sync_interval: BlockNumberFor<T>,
        /// Last confirmed sync: the matched commitment and the block of the
        /// confirmation. None if the replica hasn't confirmed a sync yet.
        last_sync: Option<ReplicaSyncRecord<BlockNumberFor<T>>>,
        /// For a virtual provider: which members of the agreement's snapshot
        /// signed `last_sync` (bitmask over the snapshot); they are the ones a
        /// failed `challenge_replica` slashes. Unused for a physical provider.
        last_sync_signers: u8,
    },
}

/// Defined in `storage_primitives`.
pub struct ReplicaSyncRecord<BlockNumber> {
    /// Commitment the replica confirmed (root + covered range).
    pub commitment: Commitment,
    /// Block of the confirmation; `min_sync_interval` counts from it.
    pub block: BlockNumber,
}

/// Defined in `storage_primitives`: the off-chain quote a provider signs and
/// the owner redeems on-chain (see `create_bucket_with_primary` /
/// `add_primary_provider` / `add_replica_provider`). The provider signs
/// `blake2_256(context | genesis_hash | SCALE(terms))`, where `context` is
/// `PRIMARY_TERM_CONTEXT` (`"primary-term-v3:"`) or `REPLICA_TERM_CONTEXT`
/// (`"replica-term-v3:"`) — domain separation between the two flavours.
///
/// The quote is a **consent token**, not a price carrier: it names *which
/// provider*, *who* may redeem, *how much* and *how long*. Every provider-side term — price,
/// replica sync price, stake (and, for virtual providers, composition) — is
/// read from `ProviderInfo` at redemption and guarded by the client's
/// `expected_version`, so there is exactly one source for those figures (see
/// "Term Pinning").
pub struct AgreementTerms<AccountId, Balance, BlockNumber> {
    /// Provider the quote binds. For a virtual provider, its synthetic
    /// account. Must equal the redeeming call's `provider`
    /// (`QuoteProviderMismatch`).
    pub provider: AccountId,
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
    /// 0 means no time-based limit (only the advancing-end check applies).
    pub min_sync_interval: BlockNumber,
}

/// Next expected `AgreementTerms.nonce` for this owner. Redemption requires an
/// exact match (`NonceMismatch`) and advances the counter by one, so a signed
/// quote is redeemable at most once, in the order it was requested. A quote
/// whose predecessor was never redeemed is re-requested; a quote costs nothing
/// until redeemed. Keyed by owner, not provider, so replay protection does not
/// depend on a provider's registration and nothing has to outlive
/// deregistration. See "Agreement Nonces" below for the entry's lifetime.
#[pallet::storage]
pub type AgreementNonces<T: Config> =
    StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

/// Pending challenges, keyed by (deadline anchor block, per-deadline index).
/// At most `MaxChallengesPerDeadline` challenges share a deadline. An expired
/// challenge stays here until `resolve_expired_challenge` slashes it.
#[pallet::storage]
pub type Challenges<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat, BlockNumberFor<T>, // deadline (anchor block)
    Twox64Concat, u16,                   // index within the deadline
    Challenge<T>,
>;

/// Per-deadline index allocator for `Challenges`. Monotone: an index is never
/// reused within a deadline. Removed when the last `Challenges` entry at that
/// deadline is removed, by any resolution path.
#[pallet::storage]
pub type NextChallengeIndex<T: Config> =
    StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, u16, ValueQuery>;

/// Challenge identifier combining deadline and index.
/// Keyed by deadline so expiry is checkable from the id alone.
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
    /// The challenged agreement.
    pub agreement_id: AgreementId,
    /// Virtual provider only: who may respond, and who is slashed on timeout.
    /// `None` for a physical provider.
    pub virtual_liability: Option<VirtualLiability<T>>,
    /// The challenged commitment, copied at creation: the snapshot's
    /// (challenge_checkpoint), the provider-signed one (challenge_offchain) or
    /// the replica's last synced one (challenge_replica).
    /// `respond_to_challenge` checks the proof at position
    /// `target.seq - start_seq` in an MMR of `leaf_count` leaves.
    pub commitment: Commitment,
    /// Which call created it; decides the valid responses (`Superseded` is
    /// valid only for `Offchain`; `Deleted` is not valid for `Replica`).
    /// Copied so verification needs no agreement.
    pub kind: ChallengeKind,
    /// Leaf + chunk being challenged (see `ChunkLocation`)
    pub target: ChunkLocation,
    /// `T::ChallengeDeposit`, held from the challenger at creation:
    ///   - valid response: returned minus the provider's reimbursed response
    ///     fee;
    ///   - `cancel_challenge`, or `resolve_expired_challenge` on a deleted
    ///     bucket: refunded in full;
    ///   - provider slashed: refunded in full, with no reward.
    pub deposit: BalanceOf<T>,
    /// Whether the challenger was authorized at challenge creation: a bucket
    /// member or the owner of the challenged agreement (`is_authorized`).
    /// Snapshotted here so that membership or agreement changes between
    /// creation and response cannot alter the fee split applied in
    /// `respond_to_challenge`.
    pub authorized: bool,
}

/// A challenge's liability against a virtual provider (virtual-provider
/// extension). Copied at creation, so the challenge needs no agreement
/// record. The agreement may be settled, extended or replaced while the
/// challenge is open.
pub struct VirtualLiability<T: Config> {
    /// The challenged agreement's `VirtualSnapshot`. Its members may respond.
    /// Its `per_provider_stake` is the slash per liable member.
    pub snapshot: VirtualSnapshot<T>,
    /// Members liable for the challenged commitment, as a bitmask over
    /// `snapshot.members`. The creating call sets it:
    ///   - `challenge_offchain`: the accepted signers, extended by
    ///     `extend_challenge`;
    ///   - `challenge_checkpoint`: the checkpoint's set bits;
    ///   - `challenge_replica`: `last_sync_signers`.
    ///
    /// `u8` bounds `MaxPhysicalMembers` to 8.
    pub liable: u8,
}

/// Which challenge call created a `Challenge`.
pub enum ChallengeKind {
    /// `challenge_checkpoint`: the snapshot at creation.
    Checkpoint,
    /// `challenge_offchain`: a signed off-chain commitment.
    Offchain,
    /// `challenge_replica`: the replica's `last_sync`.
    Replica,
}

/// Reverse index: account → buckets it is a member of. Set-membership via key
/// presence, so an account can be in **unbounded** buckets (state cost is the
/// only limit — no artificial per-account cap). Maintained on every membership
/// change; read only by the `member_buckets` runtime API (paged via
/// `iter_prefix`) and `try_state`. The member deposits pay for each entry; the
/// pallet deletes an entry when that membership ends.
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
figure until the agreement expires. `provider.stake` is only the figure used for
*new* agreements. (Price needs no such snapshot — it is prepaid at creation, so
nothing reads it later.)

The actually-locked amount is computed without ever iterating agreements:

```rust
fn locked_stake(p: &ProviderInfo) -> Balance {
    let lock = &p.higher_stake_lock;
    if lock_active(lock.until, p) { max(p.stake, lock.stake) } else { p.stake }
}

/// One predicate for every stake lock (this one, a virtual's
/// `higher_pps_lock`, a member's `Leaving` entry): a lock is active until its
/// time has passed **and** the provider it protects is not challenged. `p` is
/// that provider: the account itself, or for a virtual's `higher_pps_lock`
/// and a member's entries, the virtual. A member's entry for a dissolved
/// virtual (no `VirtualProviders` record) is inactive, whatever its `until`:
/// the caller does not call `lock_active` for it (virtual-provider
/// extension, "Dissolution").
fn lock_active(until: BlockNumber, p: &ProviderInfo) -> bool {
    now() < until || p.pending_challenges > 0
}
```

Maintenance is O(1) per event, no scan:

- **create agreement** (expiry `E`) or **create a replacement** (successor
  expiry `E`): `cur_until = max(cur_until, E)`. Both snapshot the current
  `stake`, and the snapshot is locked from creation. A successor that lapses does not lower
  `cur_until`; the figure may then exceed the latest live expiry by up to
  `additional_duration`, which the overshoot rule below already accepts.
- **raise stake:** set `stake`; nothing else. Old agreements are now *below* the
  new figure, so `locked_stake` already covers them.
- **lower stake** to `X` (`set_stake`): allowed **only if `higher_stake_lock` has
  expired** (`!lock_active(higher_stake_lock.until, p)`; trivially true for the
  default, false while the provider is challenged).
  Then fold the current generation into it —
  `higher_stake_lock = { stake, until: cur_until }` — reset `cur_until = 0` (it now tracks the new generation only), and
  set `stake = X`. The old (higher) figure stays locked while
  `lock_active(higher_stake_lock.until, p)`: until the old `cur_until`, the
  latest expiry of any agreement struck under it, and while the provider is
  challenged.
- **agreement end:** nothing.

This deliberately **overshoots** rather than track exact per-agreement maxima
(which would need an unbounded scan on end): while `higher_stake_lock` is live it
locks the whole previous generation at its top stake for its longest expiry, even
agreements that were actually cheaper or shorter. Since providers lower stake
rarely, the over-lock is a small, bounded cost for O(1) accounting. The "can't
lower again while a higher generation is still owed" rule is what keeps both
`cur_until` and `higher_stake_lock` grow-only between resets, so neither ever
needs a decrement.

The held stake is `max(stake, higher_stake_lock.stake)` until the next
`set_stake` or deregistration releases the excess of an expired lock. A
challenge keeps every lock of the account active, expired ones included, until
it is resolved: an over-lock bounded by the challenge.

**A slash** reduces `stake` and `higher_stake_lock.stake` by the slashed amount
(saturating at 0). A physical provider loses its whole held stake, so both
fields reach 0; a member loses the challenge snapshot's `per_provider_stake`. A voluntary lowering never brings
`locked_stake` below a live agreement's snapshot; only a slash does. The
agreement then continues to expiry; the owner decides on payment as usual
(`end_agreement`). A slash bumps `version` like any stake decrease ("Term
Pinning"). Every call that snapshots the stake — the three redemption calls,
`extend_agreement`, `create_replacement` — requires `stake >= MinProviderStake`,
so a provider slashed below it takes no new or extended agreement until it
raises its stake.

**Stake under open challenges.** A challenged provider cannot withdraw stake;
for a virtual provider that includes its members. Two mechanisms cover the two kinds of challenge:

- *Not yet created:* a challenge can only be created while its agreement is
  live (`now < expires_at`), so a lock that ends at the last expiry
  (`cur_until`) covers it.
- *Open:* `ProviderInfo.pending_challenges` counts the open challenges against
  the provider. Challenge creation increments it on the challenged provider
  (for a virtual provider, its synthetic account); the removal of the
  `Challenges` entry decrements it, exactly once: defended, cancelled or timed
  out. While it is above 0, `lock_active` keeps every lock of the provider
  active and `deregister_provider` is rejected. A member of a challenged
  virtual provider has its stake locked through its entry for that virtual: the entry's
  locks stay active (virtual-provider extension).

Lazy resolution does not weaken this: the stake stays until someone calls
`resolve_expired_challenge`, which anyone may do, the provider included. A
missing `ProviderInfo` counts as 0. Agreements and payments do not depend on
challenges: settlement, burn, refund, extension and replacement activation
proceed while a challenge is open, and a provider can be paid before a pending
slash is resolved. The challenge contains everything its resolution needs
(`Challenge`).

### Stake vs. capacity

**The chain enforces no relation between stake and capacity.** `max_capacity` is
self-declared and unverifiable, so it is advisory only (a "not accepting past
here" hint). `committed_bytes` — the sum of `max_bytes` over agreements the
provider accepted — is verifiable, but a stake-per-byte constraint on it
(`stake >= committed_bytes * MinStakePerByte`) would enforce nothing real.

`committed_bytes` remains as an informative figure — clients read it to judge
how loaded a provider is, and deregistration requires it to reach zero.

### Replacement agreements

The owner-only path that ends an agreement early. Virtual providers only
(`NotVirtual` otherwise): the base defines the mechanism; only the virtual-provider
extension calls it, and for a physical provider the in-place `extend_agreement`
covers every case. `create_replacement` stores a
pending successor in the agreement record (`StorageAgreement.pending_replacement`):
a fresh `agreement_id`, the provider's stake and `VirtualSnapshot` (current
member set and `per_provider_stake`) — for a replica agreement also the current
`replica_sync_price` — its expiry
`old.expires_at + additional_duration`, and the payment for
`additional_duration` at the current price, held. Creation bumps `cur_until` to
the successor's expiry ("Changeable Stake"): a later `set_stake` lowering locks
the old figure until the successor ends, not only until the old agreement does.
`additional_duration` may be 0 (a pure member-set swap). It needs no new quote
and is gated like `extend_agreement`, a zero duration included: rejected
if `expected_version` changed, if the provider is not accepting extensions
(globally, or `extensions_blocked` on this agreement), if a nonzero
`additional_duration` is below the provider's `min_duration`, if the
successor's `expires_at − now` exceeds its `max_duration`, or, for a replica
agreement, if `replica_sync_price` is `None`. The extension gates
are the provider's defence against an owner that makes a set sync without
ever activating (see below). While it
is pending, `extend_agreement`, `top_up_agreement` and
`transfer_agreement_ownership` on the old agreement are rejected
(`create_replacement`), and there is no cancel: the successor's member set may already have
synced and signed, so the owner is bound once created. It ends by activation or
with the old agreement. The in-place member-set check of `extend_agreement`
does not apply.
Pending means not live for data: no challenges, commitments naming it are not
valid until it activates, no slots in the bucket's layout. The successor's
stake snapshot is bound from creation. The old agreement runs on unchanged.

```rust
/// Successor of an agreement, stored by `create_replacement`, not yet live.
/// The successor keeps the record's `max_bytes`.
pub struct PendingReplacement<T: Config> {
    pub agreement_id: AgreementId,
    /// Provider stake at creation; becomes `StorageAgreement.stake`.
    pub stake: BalanceOf<T>,
    /// Taken at creation; at activation it is stored as `Some` in
    /// `StorageAgreement.virtual_snapshot`.
    pub virtual_snapshot: VirtualSnapshot<T>,
    /// `old.expires_at + additional_duration`, fixed at creation.
    pub expires_at: BlockNumberFor<T>,
    /// Payment for `additional_duration` at the price at creation. At
    /// activation the old agreement's unelapsed remainder is added.
    pub payment_locked: BalanceOf<T>,
    /// Replica agreement only: `replica_sync_price` at creation; becomes the
    /// successor's `sync_price`.
    pub sync_price: Option<BalanceOf<T>>,
}
```

For a primary agreement, the successor **activates** at the first
`checkpoint` that carries at least `k` of the successor's members' signatures
in the successor's pending slots (`checkpoint`). Activation runs no
`DuplicateSigner` check, because `create_replacement` already checked the
successor's accounts.
Only `checkpoint` activates a successor, never `extend_checkpoint`, because a
checkpoint attests the latest state, off-chain commitments included. The new
set must have proven that state before it takes over. Signatures over an old
snapshot prove nothing about the data since.
Activation needs a bucket admin (`checkpoint`).
Activation voids every commitment under the old `agreement_id`, including
off-chain commitments the activating checkpoint does not contain. The chain
cannot check that the activating checkpoint contains all state that matters.
The writers make sure it does: for a single writer, the activating checkpoint
contains its latest commitment; several writers coordinate as for any write
("Commit"). A writer whose commitment the
activating checkpoint does not contain commits its roots again under the
successor's `agreement_id`, with the new snapshot as `base`.
A replica is liable
for its last confirmed sync, so for a replica agreement the first
`confirm_replica_sync` with `activate_replacement` set whose signatures
satisfy the successor's snapshot activates it instead. For this one call the two rate-limit checks are replaced:
the matched commitment must advance over the old agreement's `last_sync`
(as in `confirm_replica_sync`) or equal it — the new set takes over the state the old set already
proved, with no new checkpoint needed; with no old `last_sync`, any accepted
match — and `min_sync_interval` is not applied (later syncs count from the
activating one). The old agreement's `sync_balance` and `min_sync_interval`
are kept; the activating sync becomes the successor's `last_sync` and its
signers its `last_sync_signers`. It is paid at the successor's snapshotted
`sync_price`, and emits `ReplicaSynced`, only if it
advances — an identical commitment is state already paid for. A checkpoint never
activates a replica successor, a sync never a primary one. The record becomes the
new agreement with the fixed `expires_at` and `started_at = now`; `cur_until` was bumped at creation.
The old one settles as `extend_agreement` step 1 does: elapsed
period paid to the old snapshotted members, equal split,
unelapsed remainder moved into the successor's escrow next to the payment for
`additional_duration`. The successor serves the rest of the old stretch at the
old price and the added stretch at the new one. As with an extension, the owner
pays once for every block and the provider is paid for every block served.
`create_replacement` rejects every condition that would make activation
impossible at that moment: `VirtualBelowThreshold`,
`MaxPrimarySlotsExceeded`, `DuplicateSigner`, and for a primary
`BelowMinProviders`.
Activating one pending successor never makes another impossible to activate,
because of how the `MaxPrimarySlots` and `min_providers` checks count other
pending successors (`MaxPrimarySlots`, `create_replacement`). A later change,
such as another primary's agreement expiring or ending, or an admin raising
`min_providers`, can still block activation until an admin lowers
`min_providers` or adds a primary.
Activation requires
the old agreement to be live; if it expires first, the pending successor never
activated: the old agreement settles as usual (its provider served the whole
term), the successor's payment is released to the owner (nobody served the
added stretch and nobody was challengeable for it), and `ReplacementExpired`
is emitted.
Any sync work the successor's member set did is unpaid — the graceful failure
when the new set does not sign — or, for a primary, when no bucket admin
submits the activating checkpoint. An owner can make a set sync for nothing; the
provider's defence is `accepting_extensions` / `set_extensions_blocked`, which
gate replacements too. Activation by the successor's own proof — a checkpoint
signature for a primary, a sync confirmation for a replica — is what makes the
handover safe: the new set becomes liable, and paid, only once it has proven it
holds the data, so the client never pays for a guarantee it does not hold and,
for the state the activating checkpoint contains, there is no block in which
nobody is liable. Activation writes nothing to the
members' `ProviderVirtuals` entries (virtual-provider extension, "Changing a
live agreement's member set"). Activation does not wait for challenges open
against the old agreement: each one has its own copy of the snapshot
(`Challenge.virtual_liability`), and the old members' stakes stay locked while the
virtual provider is challenged. The old set's elapsed period is paid at activation, as
on an extension.

Owner-only because activation spends the owner's escrow. A third party keeping a
frozen bucket alive funds its own replica instead (design doc "Permissionless
persistence"). The virtual-provider extension uses replacements to swap a
member set without a gap in the client's guarantee for the state the
activating checkpoint contains.

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
`replica_sync_price ↑`, `stake ↓`, or (virtual providers) a member leaving or
`k` rising; strictly-better
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

### Agreement Nonces

The client reads `AgreementNonces[owner]` from chain and sends it in
`POST /negotiate`; the provider signs it unchanged and keeps no nonce state.

- **Lifetime.** The pallet's `OnKilledAccount` impl removes the entry when the
  account is reaped, so runtimes must set
  `frame_system::Config::OnKilledAccount = StorageProvider`. A reaped owner
  restarts at nonce 0.
- **One quote at a time.** Only one quote per nonce can be redeemed. Others
  fail with `NonceMismatch`; the client reads the nonce again and requests a
  new quote. Users acting through one shared owner account, such as a
  contract, share one counter.

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

Besides the quote (`AgreementTerms`, signed with a flavour context prefix) and
the governance approvals of a virtual provider (below), two on-chain signed
payloads exist (all SCALE-encoded, all carry an explicit `version: u8` so the
protocol can evolve without breaking existing signatures, and all include the
chain's genesis hash in the preimage — "Chain binding" under
`CommitmentPayload`):

- `CommitmentPayload { version, genesis_hash, bucket_id, agreement_id, commitment }` —
  what providers sign for `/commit`, `/delete`, `checkpoint`,
  `extend_checkpoint`, `challenge_offchain` and `extend_challenge`
  (`commitment: Commitment` is defined in [Data
  Structures](#data-structures)). For `challenge_offchain` the challenger
  passes the signed `commitment` and `agreement_id` through unchanged so the
  pallet's payload reconstruction matches the signature; the challenge is
  rejected if that agreement is no longer live. `extend_checkpoint` reconstructs
  each late signer's payload the same way, from the snapshot's `commitment` and
  the `agreement_id` of the slot the entry names, so no stored per-signer
  disambiguator is needed.
- `ReplicaSyncPayload { version, genesis_hash, bucket_id, agreement_id, roots }` — what a
  replica signs for `confirm_replica_sync` to attest which roots it has
  (`roots: [Option<H256>; 7]`). `bucket_id` and `agreement_id` bind it like
  `CommitmentPayload`: usable only for that agreement, void when it ends.

The pallet verifies the governance approvals of a virtual provider in
`refund_agreement`, `set_extensions_blocked` and the virtual-provider calls.
The virtual-provider extension, "Membership Governance", defines their
format.

`confirm_replica_sync` carries an **activation flag**, `activate_replacement`.
Set, the call verifies the provider against its pending replacement's snapshot
and `agreement_id` ("Replacement agreements"); unset, against its live
agreement. The chain never tries both. With the flag set and no pending
replacement, the call fails with `NoPendingReplacement`. `checkpoint` has no
activation flag: a primary successor's members sign in pending slots
(`checkpoint`).

**Provider signatures.** Every call that takes one provider's signature over
the quote or one of these payloads takes a signature set, so the same call
serves physical and virtual providers:

```rust
/// Signatures on behalf of one provider.
pub type ProviderSignatures<T> =
    BoundedVec<(T::AccountId, Signature), T::MaxPhysicalMembers>;
```

One verification helper checks the provider signature rule for all calls:

- physical provider: exactly one entry, signed by the provider's registered
  key;
- virtual provider: at least `k` distinct members of the call's member set,
  each verified against that member's own key. Fewer rejects the call
  (virtual-provider extension).

The member set and `k` depend on the call:

- the three redemption calls: the live `members`, with the live `k`;
- `challenge_offchain` and `confirm_replica_sync`: the agreement's
  `VirtualSnapshot`, with the agreement's `k` (`VirtualSnapshot`);
- `confirm_replica_sync` with the activation flag set, overriding the case
  above: the pending successor's `VirtualSnapshot`, with the successor's `k`;
- `checkpoint` entries in pending slots: the pending successor's
  `VirtualSnapshot`, with the successor's `k`.

Three calls take signatures of several providers or add to an existing set, and
so use flat pairs instead:

- `checkpoint` — `(u8, Signature)` pairs. The `u8` names a slot of the
  bucket's current layout or a pending slot (index space and checks:
  `checkpoint`). Per provider the rule above applies, and a virtual group's
  pairs are all-or-nothing (≥`k` or none).
- `extend_checkpoint` — the same `(u8, Signature)` pairs, current layout
  only. Per provider, either a whole group at ≥`k`, or further members of a
  virtual group that already reached `k` in the current snapshot.
- `extend_challenge` — `(AccountId, Signature)` pairs: further members of the
  challenged virtual provider's snapshot, added to a challenge that already
  has ≥`k`.

**Replay & commitment validity.** A commitment is bound to one `agreement_id` and
is valid only while that agreement is live; when the agreement ends the commitment
is void. That is the whole replay model — there is no time-based nonce or recency
window. A primary prunes deleted data only once a checkpoint has raised
`start_seq` past it (the `Deleted` defense checks the snapshot). For a replica,
see "Commitment liability".

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
    /// `set_stake` set the stake for new agreements to `stake`.
    ProviderStakeChanged {
        provider: T::AccountId,
        stake: BalanceOf<T>,
    },
    ProviderSettingsUpdated {
        provider: T::AccountId,
        settings: ProviderSettings<T>,
    },
    ProviderEndpointUpdated {
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
    /// A bucket was removed and its storage deposits released.
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
    /// A primary provider was added to the bucket's provider set, by
    /// `create_bucket_with_primary`, `add_primary_provider`, or another
    /// call that creates a bucket with a primary provider. Followed by
    /// `StorageAgreementEstablished`.
    ProviderAddedToBucket {
        bucket_id: BucketId,
        provider: T::AccountId,
    },
    PrimaryProviderRemoved {
        bucket_id: BucketId,
        provider: T::AccountId,
        reason: RemovalReason,
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
    
    /// A primary agreement was opened by `create_bucket_with_primary`,
    /// `add_primary_provider`, or another call that creates a bucket with a
    /// primary provider. Always follows `ProviderAddedToBucket`.
    StorageAgreementEstablished {
        bucket_id: BucketId,
        agreement_id: AgreementId,
        provider: T::AccountId,
        owner: T::AccountId,
        terms: AgreementTerms<T>,
        payment_locked: BalanceOf<T>,
        expires_at: BlockNumberFor<T>,
    },
    /// Owner redeemed provider-signed replica terms against an existing bucket.
    ReplicaAgreementEstablished {
        bucket_id: BucketId,
        agreement_id: AgreementId,
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
    /// Owner stored a pending successor ("Replacement agreements").
    ReplacementCreated {
        bucket_id: BucketId,
        provider: T::AccountId,
        agreement_id: AgreementId,
        expires_at: BlockNumberFor<T>,
        payment_locked: BalanceOf<T>,
    },
    /// A checkpoint (primary) or a sync confirmation (replica) activated the
    /// successor: the old agreement settled
    /// (`payment_to_provider` for its elapsed period), the new one is live.
    ReplacementActivated {
        bucket_id: BucketId,
        provider: T::AccountId,
        old_agreement_id: AgreementId,
        agreement_id: AgreementId,
        expires_at: BlockNumberFor<T>,
        payment_to_provider: BalanceOf<T>,
    },
    /// The predecessor settled before the successor activated; the
    /// successor's payment returned to the owner.
    ReplacementExpired {
        bucket_id: BucketId,
        provider: T::AccountId,
        agreement_id: AgreementId,
        refunded: BalanceOf<T>,
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
    /// Provider did not respond before the deadline; the challenger's deposit
    /// was refunded. Physical provider: its whole stake went to `T::OnSlash`.
    /// Virtual provider (`provider` is the virtual's account): each member in
    /// `virtual_liability.liable` lost `per_provider_stake`. `slashed_amount`
    /// is the total.
    ChallengeSlashed {
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        provider: T::AccountId,
        slashed_amount: BalanceOf<T>,
    },
    /// The challenge ended without a response and without a slash:
    /// `cancel_challenge`, or `resolve_expired_challenge` on a deleted bucket.
    /// The challenger's deposit was refunded in full.
    ChallengeCancelled {
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        provider: T::AccountId,
    },
    /// `extend_challenge` added members to a challenge's
    /// `virtual_liability.liable`. `liable` is the new bitmask.
    ChallengeExtended {
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        liable: u8,
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
        /// Contains `member_count` and `admin_count`, not the member list.
        fn bucket_info(bucket_id: BucketId) -> Option<BucketResponse>;
        /// A bucket's members and roles, at most `limit`, in `BucketMembers`
        /// key order, starting after `start_after` (prefix iteration over the
        /// bucket). For the next page, pass the last account of this page as
        /// `start_after`.
        fn bucket_members(bucket_id: BucketId, start_after: Option<AccountId>, limit: u32) -> Vec<(AccountId, Role)>;
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
        /// Challenges the provider can still answer; expired ones are omitted.
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
    /// `version = 0`, `last_stake_event = None`.
    ///
    /// Also holds the provider record's storage deposit
    /// (`ProviderInfo.deposit`) on the caller, beside the stake.
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
    /// `!lock_active(higher_stake_lock.until, p)`: the time has passed and `pending_challenges == 0`) and the new value is `>= MinProviderStake`;
    /// otherwise `StakeStillLocked` / `StakeBelowMinimum`. A member of a
    /// virtual provider also cannot go below what its `ProviderVirtuals` entries
    /// require (virtual-provider extension). It prunes the caller's `Leaving`
    /// entries that are no longer active and releases their
    /// `VirtualMembership.deposit`. Existing
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
    /// A provider offboards by not accepting new agreements/extensions and
    /// letting its existing ones run to expiry. Fails while
    /// `committed_bytes > 0` (agreements not yet settled), while
    /// `pending_challenges > 0`, and, for a member, while it has an `Active`
    /// entry or a `Leaving` entry whose lock is active (virtual-provider
    /// extension). Once these checks pass no challenge can be open or created
    /// (every challenge path requires a live agreement), so the stake is
    /// released immediately; no announcement window is needed ("Changeable
    /// Stake").
    ///
    /// Removes the `Providers` entry and releases its storage deposit
    /// (`ProviderInfo.deposit`). Also prunes the caller's `Leaving` entries
    /// whose lock is no longer active and releases their
    /// `VirtualMembership.deposit` (virtual-provider extension). Quote replay
    /// protection is per owner (`AgreementNonces`), so a quote already
    /// redeemed stays unredeemable against a later re-registration of the same
    /// key without any retained provider state.
    #[pallet::weight(...)]
    pub fn deregister_provider(origin: OriginFor<T>) -> DispatchResult;

    /// Update provider settings.
    /// 
    /// Allows provider to change pricing, duration limits, capacity,
    /// minimum agreement size, and availability. Changes apply to new
    /// agreements only; existing agreements retain their locked terms.
    ///
    /// Validation:
    /// - `min_duration <= max_duration` (`MinDurationExceedsMaxDuration`).
    /// - If `max_capacity > 0`: must be `>= committed_bytes`
    ///   (`CapacityBelowCommitted`). `max_capacity` is a self-declared *advisory*
    ///   ceiling only.
    /// - If `max_capacity > 0`: `min_bytes` must be `<= max_capacity`
    ///   (`MinBytesExceedsMaxCapacity`).
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
    /// Rejected for a `Virtual` provider (`NotPhysical`), because a virtual
    /// provider has no endpoint of its own (virtual-provider extension). Emits
    /// `ProviderEndpointUpdated`.
    #[pallet::weight(...)]
    pub fn update_provider_endpoint(
        origin: OriginFor<T>,
        multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
    ) -> DispatchResult;

    /// Block or unblock extensions for a specific bucket (provider only).
    /// Allows provider to stop a specific bucket from extending while
    /// continuing to accept extensions from other buckets. Also blocks
    /// `create_replacement`.
    ///
    /// Origin: the provider, with `approvals` empty; for a virtual provider,
    /// any live member, with `approvals` carrying member signatures that
    /// together hold a seniority majority (approval format in
    /// virtual-provider extension, "Membership Governance").
    ///
    /// Requires a registered provider (`ProviderNotFound`) with a live
    /// agreement on the bucket (`AgreementNotFound`, `AgreementExpired`).
    #[pallet::weight(...)]
    pub fn set_extensions_blocked(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        blocked: bool,
        approvals: BoundedVec<(T::AccountId, Signature), T::MaxPhysicalMembers>,
    ) -> DispatchResult;




    // ─────────────────────────────────────────────────────────────
    // Bucket management
    // ─────────────────────────────────────────────────────────────

    /// Create a new bucket.
    /// 
    /// The caller becomes the bucket admin and pays two storage deposits: one
    /// for the bucket record (`Bucket.deposit`) and one for its own member
    /// entry (`Member.deposit`). The bucket starts empty with no providers or
    /// data.
    /// 
    /// Parameters:
    /// - `min_providers`: Minimum signed slots required for checkpoints.
    ///   Accepts 1 to `T::MaxPrimarySlots`; rejects other values
    ///   (`InvalidMinProviders`).
    /// - `visibility`: `Public` or `Private` (see `Visibility`). Wrappers that
    ///   omit the choice must default to `Private` (fail-safe: an unset choice
    ///   should protect data, not expose it).
    #[pallet::weight(...)]
    pub fn create_bucket(
        origin: OriginFor<T>,
        min_providers: u32,
        visibility: Visibility,
    ) -> DispatchResult;

    /// Delete an empty bucket and release its storage deposits.
    ///
    /// Requires:
    ///   - no storage agreement on the bucket (`BucketNotEmpty`);
    ///   - the caller is the bucket's only member (`member_count == 1`, else
    ///     `BucketHasMembers`). The only member is also the only admin.
    ///
    /// Every other member must be removed first. Other admins remove
    /// themselves. Open challenges do not block deletion: a challenge on a
    /// deleted bucket slashes nothing (`resolve_expired_challenge`).
    ///
    /// Removes the caller's `BucketMembers` and `MemberBuckets` entries and
    /// releases the caller's member deposit to the member entry's
    /// `depositor`. Then removes the bucket and releases the bucket deposit to
    /// `Bucket.creator`. While the creator is an admin, the refund gives the
    /// creator a reason to delete a bucket nobody uses.
    /// Emits `BucketDeleted`.
    #[pallet::weight(...)]
    pub fn delete_bucket(
        origin: OriginFor<T>,
        bucket_id: BucketId,
    ) -> DispatchResult;

    /// Set minimum providers required for checkpoint (admin only).
    /// 
    /// Controls redundancy: checkpoints require at least this many signed
    /// slots. Accepts 1 to `T::MaxPrimarySlots`, independent of the current
    /// layout; rejects other values (`InvalidMinProviders`). While the layout
    /// has fewer slots of live agreements, no checkpoint is possible. When
    /// primary agreements expire or end and leave fewer live slots, the admin
    /// lowers the value or adds a primary.
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket to modify
    /// - `min_providers`: New minimum count of signed slots for checkpoints
    #[pallet::weight(...)]
    pub fn set_min_providers(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        min_providers: u32,
    ) -> DispatchResult;

    /// Freeze bucket — make append-only (admin only, irreversible)
    /// Requires the current snapshot to meet `min_providers`. The call
    /// recounts the snapshot's set `primary_signers` bits, counting only bits
    /// in slots of agreements that have not expired.
    /// Fails with `NoSnapshot` before the first checkpoint, and with
    /// `MinProvidersNotMet` when fewer counted bits than `min_providers`
    /// remain: submit a new checkpoint with enough live signers, or lower
    /// `min_providers`.
    #[pallet::weight(...)]
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
    /// The bucket's only admin (`admin_count == 1`) cannot demote itself
    /// (`LastAdminCannotBeRemoved`). A bucket always keeps ≥ 1 admin.
    /// An admin that owns a primary agreement on the bucket, live or waiting
    /// for settlement, cannot demote itself (`OwnsPrimaryAgreement`). The
    /// check reads the owner of each agreement in `primary_providers` (at most
    /// `MaxPrimarySlots` reads). To get past it, the admin transfers the
    /// agreement to another admin (`transfer_agreement_ownership`; with a
    /// replacement pending, activate the successor first) or, after expiry,
    /// settles it (`end_agreement`, or `claim_expired_agreement` after
    /// `SettlementTimeout`).
    /// 
    /// This prevents a single compromised admin from seizing control.
    ///
    /// Adding a `Reader` is what makes membership the read access list for a
    /// private bucket. Visibility is set separately via `set_bucket_visibility`—
    /// adding a Reader does not by itself make a bucket private.
    ///
    /// Adding a member:
    ///   - holds a storage deposit (`Member.deposit`) on the caller, not on
    ///     the member;
    ///   - writes the member's `BucketMembers` and `MemberBuckets` entries;
    ///   - increments `Bucket.member_count`.
    ///
    /// Adding a member is rejected at `member_count == MaxMembers`
    /// (`TooManyMembers`). A role change creates no record and holds nothing.
    /// Both adding a member and a role change update `Bucket.admin_count`
    /// when the member becomes or stops being an admin.
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
    /// - Remove themselves, except:
    ///   - the bucket's only admin (`LastAdminCannotBeRemoved`; a bucket
    ///     always keeps ≥ 1 admin);
    ///   - an admin that owns a primary agreement on the bucket
    ///     (`OwnsPrimaryAgreement`, as in `set_member`).
    /// 
    /// This prevents a single compromised admin from seizing control.
    /// 
    /// Note: This is a very primitive handling of multiple admin accounts, in
    /// practice you should be very careful with adding such accounts and should
    /// lean towards using a single one controlled by a DAO (contract, chain,
    /// ..).
    ///
    /// Removes the member's `BucketMembers` and `MemberBuckets` entries.
    /// Decrements `Bucket.member_count`, and `admin_count` for an admin.
    /// Releases the storage deposit to the admin that added the member
    /// (`Member.depositor`), whoever performs the removal.
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
    // submits them in a single call. Every call that inserts an agreement
    // also holds its storage deposit on the owner, next to the payment
    // (`StorageAgreement.deposit`). `create_bucket_with_primary`,
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
    // - `terms.provider` must match the call's `provider`
    //   (`QuoteProviderMismatch`)
    // - `terms.max_bytes > 0` (`InvalidMaxBytesRequest`)
    // - `now <= terms.valid_until <= now + T::RequestTimeout`
    //   (`TermsExpired` / `TermsValidityTooLong`)
    // - `sigs` must satisfy the provider signature rule (see
    //   `ProviderSignatures`) over
    //   `blake2_256(context | genesis_hash | SCALE(terms))` with the
    //   flavour's context; for a virtual provider the member set is its
    //   live `members`, which the new agreement snapshots
    // - `terms.nonce` must equal the owner's next expected agreement nonce
    //   (`NonceMismatch`; see `AgreementNonces`)
    // - `expected_version == provider.version`
    //   (`ProviderVersionMismatch`; see "Term Pinning")
    // - the provider must be registered with `stake >= MinProviderStake`
    //   (`StakeBelowMinimum`), within its duration bounds, and the added `terms.max_bytes` must fit
    //   its declared capacity (`CapacityExceeded`)
    // - `terms.max_bytes` must be `>= settings.min_bytes` (`MaxBytesBelowMinimum`)
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
    ///   takes one slot per member, and a primary with a pending successor
    ///   counts as `MaxPrimarySlots` describes
    /// - no account the provider adds (itself, or for a virtual provider its
    ///   live members) may already be in the bucket's slot layout or in a
    ///   pending successor's snapshot (`DuplicateSigner`; see
    ///   `BucketSnapshot.primary_signers`)
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
    /// Fails with `ReplacementPending` while a replacement is pending (its
    /// payment was computed for the current `max_bytes`).
    #[pallet::weight(...)]
    pub fn top_up_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        additional_bytes: u64,
        expected_version: u32,
    ) -> DispatchResult;

    /// Store a pending successor for a live agreement (**owner only**,
    /// virtual providers only — `NotVirtual` otherwise); see
    /// "Replacement agreements". Gated like `extend_agreement`:
    /// `expected_version`, accepting extensions (global and per agreement),
    /// `min_duration` on a nonzero `additional_duration`, `max_duration` on
    /// the successor's `expires_at − now`. No new quote. `additional_duration`
    /// may be 0: a pure member-set swap.
    /// Until the successor activates or the agreement settles, these calls on
    /// this agreement fail with `ReplacementPending`:
    ///   - `extend_agreement`, `top_up_agreement`: the successor's expiry and
    ///     payment are fixed against the current term;
    ///   - `transfer_agreement_ownership`.
    ///
    /// `top_up_replica_sync_balance` stays allowed; the balance is kept.
    /// The successor cannot be cancelled. Locks the payment for
    /// `additional_duration` at the current price. Bumps the provider's
    /// `cur_until` to the successor's expiry ("Changeable Stake"). For a
    /// replica agreement also snapshots the current `replica_sync_price`;
    /// such a successor activates by `confirm_replica_sync`, not `checkpoint`.
    /// Fails if:
    ///   - the caller is not the agreement's owner (`NotAgreementOwner`);
    ///   - the agreement has expired (`AgreementExpired`);
    ///   - a replacement is already pending (`ReplacementPending`);
    ///   - the provider's `stake < MinProviderStake` (`StakeBelowMinimum`);
    ///   - on a replica agreement, the provider's `replica_sync_price` is
    ///     `None` (`ProviderNotAcceptingReplicas`);
    ///   - the virtual's live `members.len() < k` (`VirtualBelowThreshold`),
    ///     as agreement creation does;
    ///   - on a primary agreement, the successor's members would push the
    ///     bucket's slot layout past `T::MaxPrimarySlots`
    ///     (`MaxPrimarySlotsExceeded`; counted as `MaxPrimarySlots` describes);
    ///   - on a primary agreement, the layout would have fewer slots than
    ///     `min_providers` (`BelowMinProviders`): lower `min_providers` or add
    ///     a primary first. The check counts the successor in place of the
    ///     current set, leaves out every primary whose agreement has expired,
    ///     its pending successor included, and counts every other primary with
    ///     a pending successor at the smaller of its current slots and its
    ///     successor's members, so that activating one successor never makes
    ///     another impossible to activate;
    ///   - on a primary agreement, a member of the successor snapshot is in a
    ///     slot of another primary or in another primary's pending successor
    ///     snapshot (`DuplicateSigner`; see `BucketSnapshot.primary_signers`).
    ///
    /// The successor activates only while this agreement is live. Otherwise
    /// it lapses when this agreement settles, and its payment is released to
    /// the owner.
    pub fn create_replacement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        additional_duration: BlockNumberFor<T>,
        expected_version: u32,
    ) -> DispatchResult;

    /// Extend agreement duration (**owner only**).
    /// Only while the agreement is live — an expired one settles via
    /// `end_agreement` / `claim_expired_agreement`, never here.
    /// 1. Settles the elapsed period: pays the provider
    ///    `payment_locked × (now − started_at) / (expires_at − started_at)` out
    ///    of escrow and sets `started_at = now`. Pro-rata of the escrow, so no
    ///    price is stored and an escrow that mixes stretches at different
    ///    prices settles exactly over the whole term.
    /// 2. Locks new payment for `additional_duration` at the current price;
    ///    the unelapsed remainder stays in escrow for the original stretch
    /// 3. `expires_at += additional_duration`
    /// 4. Re-snapshots the provider's current terms into the agreement
    ///
    /// The owner pays once for every block. Step 1 pays the provider at every
    /// extension, and the remaining term after the call is bounded (below), so
    /// the provider is paid at least once per `max_duration` regardless of how
    /// many extensions.
    ///
    /// **Owner-only** — extension is NOT permissionless. Step 1 pays the elapsed
    /// portion out to the provider, so if a third party (or the provider itself)
    /// could extend, it could force-settle the elapsed term and defer expiry
    /// indefinitely, stripping the owner of its burn/exit lever
    /// (design doc "The Burn Option"). Only the
    /// owner may spend its own locked payment this way, so only the owner extends.
    /// The `expected_version` pin still applies (owner pays no more than it saw).
    ///
    /// For a virtual provider, the call is rejected if:
    ///   - the live member set differs from the agreement's snapshot
    ///     (`VirtualMembersChanged`);
    ///   - the live `stake / per_provider_stake` is above the agreement's `k`
    ///     (`VirtualThresholdRaised`).
    ///
    /// In those cases the owner creates a replacement. The snapshot's
    /// `members` stay as stored. Only `stake` and `per_provider_stake` are
    /// re-snapshotted, so the slot layout does not change (virtual-provider
    /// extension, "Changing a live agreement's member set").
    ///
    /// Ending an agreement early, against a proven successor, is
    /// `create_replacement` plus activation ("Replacement agreements"), also
    /// owner-only. A third party keeps a frozen/public bucket alive by funding
    /// its own replica, never by touching the owner's agreement (design doc
    /// "Permissionless persistence").
    ///
    /// Also fails if:
    /// - The agreement has expired (`AgreementExpired`)
    /// - The provider's `stake < MinProviderStake` (`StakeBelowMinimum`)
    /// - A replacement is pending (`ReplacementPending`): its expiry and
    ///   payment were fixed against this agreement's current term
    /// - `additional_duration` is below the provider's `min_duration`, or the
    ///   remaining term after the call, `expires_at − now`, is above its
    ///   `max_duration`
    /// - Provider has globally paused extensions (settings.accepting_extensions == false)
    /// - Provider has blocked extensions for this specific bucket (agreement.extensions_blocked == true)
    /// - `expected_version != provider.version` (`ProviderVersionMismatch`)
    /// - On a replica agreement, the provider's `replica_sync_price` is `None`
    ///   (`ProviderNotAcceptingReplicas`): there is no sync price to
    ///   re-snapshot
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
    /// Hands a primary agreement to another admin of the bucket, or a replica
    /// agreement to any account. The new owner can top up quota and transfer
    /// ownership further.
    ///
    /// **A primary agreement's owner is always a bucket admin.** For a primary
    /// agreement, `new_owner` must be an admin of the bucket
    /// (`NewOwnerNotAdmin`), and an admin that owns a primary agreement cannot
    /// demote or remove itself (`set_member`, `remove_member`). The owner
    /// decides extension, replacement and pay or burn, so these decisions stay
    /// with the bucket's admins. A DAO, contract or multisig takes over a
    /// primary by becoming an admin first.
    ///
    /// Rejected while a replacement is pending (`ReplacementPending`): the
    /// successor's payment is held on the current owner and does not move.
    ///
    /// **The escrow moves with the agreement.** The prepaid fee, and a
    /// replica's unspent sync balance, are held on the owner, so the hold
    /// moves to `new_owner` and stays a hold. Every later settlement and
    /// refund then uses the new owner.
    ///
    /// **Bucket membership does not change.** A transfer adds or removes no
    /// member. A replica's new owner gets no access to the bucket.
    ///
    /// **Challenge rights follow the owner.** The new owner becomes an
    /// authorized challenger of this agreement's provider. Open challenges
    /// keep the tier they were created with.
    ///
    /// **The storage deposit does not move.** It stays on
    /// `StorageAgreement.depositor` until the agreement is removed, so
    /// `new_owner` pays nothing. After the transfer the depositor no longer
    /// decides when that happens.
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
    /// settle. If owner doesn't act, anyone can call claim_expired_agreement
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
    /// Early termination of primaries by the admin is an open question (see
    /// "Open Questions").
    /// 
    /// **Replicas cannot be ended early here:** the owner-only replacement
    /// ("Replacement agreements") is the one early end, as for primaries. Allowing
    /// it here would violate the principle of least surprise. A business checking on a
    /// bucket sees "5 providers with agreements until May" and concludes all is
    /// well - they shouldn't find the bucket dead the next day because someone
    /// terminated agreements early. If unhappy with a provider, simply don't extend.
    /// 
    /// Note: Admin has no special privileges over replica agreements.
    #[pallet::weight(...)]
    pub fn end_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        action: EndAction,
    ) -> DispatchResult;

    /// Settle an expired agreement with payment to the provider (anyone can
    /// call).
    /// Can only be called after agreement expired + T::SettlementTimeout
    /// (`AgreementNotExpired`). The owner forfeited its right to burn by not
    /// acting in time, so the outcome is fixed and the caller decides
    /// nothing. Payment goes to the provider, or for a virtual provider to its
    /// snapshotted members, as in `end_agreement` with pay. Open to anyone:
    /// a virtual provider's account has no key, and other accounts depend on
    /// the settlement: the depositor gets its deposit back, the owner gets a
    /// lapsed successor's payment back, and for a primary the admins get the
    /// slot back. The caller receives nothing.
    ///
    /// Effects, as for any settlement: removes the agreement record and, for
    /// a primary, the provider's entry in `primary_providers`, so the slot
    /// layout shrinks; lowers the provider's `committed_bytes` by the
    /// agreement's `max_bytes`; releases the storage deposit to `depositor`; a
    /// pending successor lapses (`ReplacementExpired`, its payment released
    /// to the owner). Emits `AgreementExpiredClaimed`.
    #[pallet::weight(...)]
    pub fn claim_expired_agreement(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
    ) -> DispatchResult;

    /// Give the agreement's remaining payment back to its owner (**provider
    /// only**).
    ///
    /// Origin: the provider, with `approvals` empty; for a virtual provider,
    /// any live member, with `approvals` carrying member signatures that
    /// approve the refund and together hold a seniority majority
    /// (virtual-provider extension, "Payment"; approval format in
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

    // ─────────────────────────────────────────────────────────────
    // Checkpoints
    // ─────────────────────────────────────────────────────────────

    /// Submit a new checkpoint with provider signatures (writers/admin only;
    /// admin for a deletion or an activation).
    /// 
    /// Sets the canonical state and its signers, and activates each pending
    /// replacement whose group signs in its pending slots ("Replacement
    /// agreements").
    ///
    /// Index space of `signatures`: the slots of the current layout, at
    /// their indices, followed by the **pending slots**. For each virtual
    /// whose agreement has a pending replacement, in `primary_providers`
    /// order, the pending slots are the members of the successor's
    /// snapshot, in snapshot order. A virtual has pending slots while
    /// `pending_replacement` is `Some`, also after its agreement expired.
    /// The pallet verifies an entry in a pending slot against that member's
    /// key over the `CommitmentPayload` naming the successor's
    /// `agreement_id`.
    ///
    /// Checks and effects:
    ///   - The call rejects an index at or past the end of the index space
    ///     (`InvalidSlot`) and an index repeated in the call (`DuplicateSlot`).
    ///     The check is per index: a member of both a virtual's current slots
    ///     and its successor's snapshot has one index in each.
    ///   - A virtual group's signatures in one call are all-or-nothing: at
    ///     least its `k`, or none. Fewer rejects the call (virtual-provider
    ///     extension, "Checkpoints"). A group is a virtual's current
    ///     slots, with its agreement's `k`, or a successor's pending slots,
    ///     with the successor's `k` (`VirtualSnapshot`).
    ///   - The call rejects entries in a virtual's current slots if it also
    ///     contains that virtual's pending group (`ConflictingSigners`). Each
    ///     virtual therefore contributes at most the larger of its current
    ///     slot count and its successor's member count. The slot count check
    ///     (`MaxPrimarySlots`) bounds the sum, so `signatures` fits in
    ///     `T::MaxPrimarySlots`.
    ///   - A call that contains a pending group requires a bucket admin
    ///     (`NotBucketAdmin`): activation voids every commitment under the old
    ///     `agreement_id`, so state the activating checkpoint does not contain
    ///     loses its liable providers. As for a deletion, only an admin may
    ///     decide that. Other checkpoints take a bucket writer or admin.
    ///   - The call rejects an entry in a slot whose agreement has expired,
    ///     and a pending group whose old agreement has expired
    ///     (`AgreementExpired`): an expired agreement cannot be challenged, so
    ///     its signature would count toward `min_providers` without liability.
    ///   - A successor without entries stays pending, and its virtual's
    ///     current slots sign as usual.
    ///   - The final layout is the current layout with each activated
    ///     successor's members in place of its virtual's slots, at that
    ///     virtual's position. The pallet requires at least `min_providers`
    ///     set bits over the final layout (`InsufficientSignatures`).
    ///   - The call stores `primary_signers` over the final layout only
    ///     (`BucketSnapshot.primary_signers`).
    ///
    /// The commitment must be identical to the current snapshot (same root,
    /// `start_seq` and `leaf_count`) or **advance** over it in one of two ways:
    /// - **Append**: same `start_seq`, larger `leaf_count`, and
    ///   `consistency_proof` shows that the snapshot's MMR is a prefix of the
    ///   new one (`MmrConsistencyProof`). A proof that fails verification
    ///   rejects the call (`InvalidConsistencyProof`). Every earlier canonical
    ///   state of the MMR is therefore a prefix of the current one.
    /// - **Deletion** (admin only): `start_seq` equal to the snapshot's end
    ///   (`start_seq + leaf_count`), no proof. Exception: when the snapshot's
    ///   `leaf_count` is 0 (no snapshot, or after a delete-all), the end
    ///   equals `start_seq`. A commitment at that `start_seq` is then an
    ///   append, which writers may submit, not a deletion.
    ///   A deletion starts a new MMR. The new MMR contains the leaves the
    ///   admin keeps, appended again in their original order with new
    ///   sequence numbers, so every primary builds the same root. The chain
    ///   cannot check the kept leaves. The admin checks the root before
    ///   submitting. A canonical sequence number is never reused, so every
    ///   leaf of an earlier canonical MMR is below the new `start_seq`
    ///   (`Deleted`).
    ///
    /// Every checkpoint, an identical one included, meets the signature rules
    /// on its own and replaces the snapshot's `primary_signers`.
    /// A bucket without a snapshot counts as an empty MMR at `start_seq` 0
    /// (for the empty-MMR root, see `Commitment`). Rejects a commitment with
    /// `leaf_count == 0` whose `mmr_root` is not the empty root for its
    /// `start_seq` (`InvalidEmptyRoot`). Rejects a commitment whose
    /// `start_seq + leaf_count` overflows `u64`.
    /// The end (`start_seq + leaf_count`) never decreases, so a leaf the
    /// pruning rule lets a provider delete never becomes canonical.
    /// A primary prunes deleted data only once a deletion checkpoint is
    /// on-chain. For a replica, see "Commitment liability".
    /// A commitment with `leaf_count` above 2^48 does not decode (`LeafCount`).
    /// For frozen buckets: start_seq must equal frozen_start_seq (append only).
    pub fn checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        commitment: Commitment,
        // Required for an append to a non-empty snapshot; must be `None`
        // otherwise.
        consistency_proof: Option<MmrConsistencyProof>,
        // One entry per signing slot: the index of a current or pending slot
        // (index space above) and the slot account's signature over the
        // `CommitmentPayload` naming that slot's `agreement_id` ("Provider
        // signatures").
        signatures: BoundedVec<(u8, Signature), T::MaxPrimarySlots>,
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
    /// virtual primary, the call contains either its whole group at ≥`k`, or
    /// further members of a group that already reached `k` in this snapshot
    /// (virtual-provider extension, "Checkpoints"). Slot indices refer to the
    /// current layout only: the call rejects an index at or past its end, a
    /// pending-slot index included (`InvalidSlot`), and an index repeated in
    /// the call (`DuplicateSlot`). It rejects an entry in a slot whose
    /// agreement has expired (`AgreementExpired`), as `checkpoint` does.
    /// Never activates a pending replacement: only a fresh `checkpoint` does
    /// ("Replacement agreements").
    pub fn extend_checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        // Slot index and signature, as in `checkpoint`.
        additional_signatures: BoundedVec<(u8, Signature), T::MaxPrimarySlots>,
    ) -> DispatchResult;

    /// Add members to an open off-chain challenge against a virtual provider
    /// (anyone can call).
    ///
    /// Verifies further signatures over the challenged `CommitmentPayload` from
    /// members of the challenge's `virtual_liability.snapshot`, and adds the
    /// signers to `virtual_liability.liable`. Emits `ChallengeExtended`.
    /// Fails if:
    ///   - the challenge does not exist (`ChallengeNotFound`);
    ///   - its deadline has passed (`ChallengeExpired`);
    ///   - it is not an off-chain challenge against a virtual provider
    ///     (`ChallengeNotExtendable`). A physical provider has no further
    ///     signers. A checkpoint challenge takes `liable` from the
    ///     checkpoint's set bits, which `extend_checkpoint` extends. A replica
    ///     challenge's signers signed a `ReplicaSyncPayload`;
    ///   - a signer is not in `virtual_liability.snapshot`, or its signature
    ///     does not verify (`InvalidSignature`).
    ///
    /// Permissionless like
    /// `extend_checkpoint`, because it only adds accountability. Without this
    /// call, a challenger could present the minimum `k` signatures and leave
    /// the other signers off the liable set (virtual-provider extension,
    /// "Stake and Slashing").
    pub fn extend_challenge(
        origin: OriginFor<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
        additional_signatures: BoundedVec<(T::AccountId, Signature), T::MaxPhysicalMembers>,
    ) -> DispatchResult;

    // ─────────────────────────────────────────────────────────────
    // Challenges
    // ─────────────────────────────────────────────────────────────
    //
    // Three challenge modes:
    //
    // **challenge_checkpoint**: against the current snapshot, with no
    // signatures. A primary with a set bit in the snapshot's
    // `primary_signers` is liable for every leaf the snapshot contains.
    //
    // **challenge_offchain**: against a commitment the challenger holds
    // signatures for, on a leaf past the snapshot's end or in a bucket
    // without a snapshot. For a leaf the snapshot covers, use
    // challenge_checkpoint.
    //
    // **challenge_replica**: against the replica's confirmed `last_sync`,
    // with no signatures (for a virtual provider, `last_sync_signers` are
    // the ones slashed).
    //
    // **Who may challenge, and at what cost (all three modes):**
    // Any signed account may challenge, including the challenged provider and
    // the members of a challenged virtual provider. One exception: on a
    // `Private` bucket, challenging a provider whose agreement role is
    // `Primary` requires bucket membership (a point read of
    // `BucketMembers(bucket, who)`), else
    // `NotAuthorizedForPrivateBucket` (rationale in the design doc, "The
    // Challenge Game"). The gate reads the challenged provider's role from
    // its *current* agreement, the same lookup that yields
    // `AgreementNotFound`. An ended agreement therefore means no challenge,
    // never a stale role.
    // In all three modes the target leaf must exist: `target.seq` must lie in
    // `[start_seq, start_seq + leaf_count)` of the commitment, else
    // `LeafNotInCommitment`.
    // Without this check anyone could slash any provider: no response exists
    // for a leaf the commitment does not contain, so the challenge would time
    // out. All three calls also reject a
    // commitment whose `start_seq + leaf_count` overflows `u64`, as
    // `checkpoint` does.
    // `T::ChallengeDeposit` is held from the challenger. The excess is
    // refunded on resolution. On a valid response the provider's stake is
    // never touched. Only its response transaction fee is at issue, and the
    // deposit reimburses it. How much of that cost the provider is made to bear
    // depends on the challenger:
    //
    //   - **Authorized accounts** — `is_authorized(who, bucket, provider)` is
    //     true for a bucket member (Admin/Writer/Reader; a point read of
    //     `BucketMembers(bucket, who)`) or the owner of the challenged
    //     agreement (`StorageAgreements(bucket, provider).owner`, one read).
    //     For a virtual provider, the challenged agreement is the one keyed by
    //     its synthetic account. The provider is made to bear a fraction of
    //     the cost per the cost-split table (response-time based); the
    //     challenger's deposit covers the rest. The
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
    // `is_authorized` is the predicate for the tier. No per-challenge rate
    // limiting or
    // stored "last challenge" timestamp is needed: full-cost public challenges
    // are self-limiting (the challenger pays in full every time) and leave an
    // honest provider financially unharmed.

    /// Challenge on-chain checkpoint (no signatures needed).
    /// Provider must be in current snapshot's provider list **and** have a live
    /// agreement for this bucket (`AgreementNotFound` if it has none,
    /// `AgreementExpired` if it has expired; live means `now < expires_at`).
    /// Liability tracks the agreement, not lingering snapshot membership: once
    /// a provider's last agreement ends no new challenge against it can be
    /// created — even if it is still named in an un-superseded snapshot — and
    /// its open ones keep its stake locked through `pending_challenges`.
    /// On a `Private` bucket the caller must be a member
    /// (`NotAuthorizedForPrivateBucket`).
    /// Snapshot providers are primaries by construction, so the gate always
    /// applies here.
    /// 
    /// A new checkpoint can be included between submission and execution.
    /// The call then uses the new snapshot's commitment, and `target.seq`
    /// still names the same leaf. The call fails if a deletion removed the
    /// leaf (`LeafNotInCommitment`), or if the new snapshot has no set bit for
    /// the provider (`ProviderNotInSnapshot`): the provider is then not
    /// liable for the leaves that snapshot covers.
    pub fn challenge_checkpoint(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        target: ChunkLocation,
    ) -> DispatchResult;

    /// Challenge off-chain commitment (requires the provider's signatures).
    /// The signatures prove that the provider committed to this data.
    /// `provider_signatures` must satisfy the provider signature rule
    /// (`ProviderSignatures`) over the
    /// `CommitmentPayload`. For a virtual provider, the member set is the
    /// agreement's snapshot, and the accepted signers form the challenge's
    /// `virtual_liability.liable` (`extend_challenge` adds more).
    /// On a `Private` bucket, if the challenged provider's current agreement
    /// has role `Primary`, the caller must be a member
    /// (`NotAuthorizedForPrivateBucket`; role-based gate, see above).
    /// Rejected if `agreement_id` is not the provider's agreement for this
    /// bucket (`AgreementNotFound`) or that agreement has expired
    /// (`AgreementExpired`) — a commitment is void once its agreement ends, so
    /// obsolete signatures can't be used to slash. Also rejected if
    /// `target.seq` is below the current snapshot's end (`start_seq +
    /// leaf_count`): below its `start_seq`, because a deletion removed the
    /// leaf (`ChallengedLeafDeleted`), otherwise (`LeafInSnapshot`). For a
    /// covered leaf, use `challenge_checkpoint`. If the provider has no set
    /// bit in the snapshot, first add its signature over the snapshot's
    /// `CommitmentPayload` with `extend_checkpoint`.
    /// A commitment with `leaf_count` above 2^48 does not decode (`LeafCount`).
    pub fn challenge_offchain(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        agreement_id: AgreementId,
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
    /// The challenged leaf may be below the snapshot's `start_seq`: a replica
    /// is liable for exactly its `last_sync`, so a deletion does not end that
    /// liability until the replica syncs again.
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
    /// provider, by any member of the selected snapshot: the agreement's, or
    /// with `activate_replacement` set the pending successor's. With
    /// `activate_replacement` set the signatures are verified against the
    /// pending successor's snapshot and `agreement_id` and activate it
    /// ("Replacement agreements").
    /// Rejected once the agreement has expired (`AgreementExpired`): an expired
    /// replica is no longer challengeable, so it is not paid for syncs either.
    /// Only one sync per agreement is kept in the pool at a time; concurrent
    /// submissions by other members are dropped there and never charged
    /// ("Transaction extension for raced calls").
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
    /// 1. The matched commitment must **advance** over `last_sync.commitment`:
    ///    a larger `leaf_count` at the same
    ///    `start_seq`, or a larger `start_seq`, so a deletion is confirmable
    ///    and paid. Both are canonical, so no proof is needed. The same
    ///    commitment is never paid twice
    /// 2. `current_block >= last_sync.block + min_sync_interval` (per-agreement)
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
    /// - Updates replica's `last_sync` to the matched commitment and `current_block`;
    ///   a historical match records that entry's full commitment, so
    ///   `challenge_replica` can bind a proof to it as well
    /// - Pays sync_price from replica's sync_balance
    /// - Emits ReplicaSynced event with position_matched for performance tracking
    ///   (position 0 = current snapshot, 1-6 = historical positions, higher = more lag)
    /// 
    /// Parameters:
    /// - `bucket_id`: The bucket the replica is syncing
    /// - `provider`: The replica provider (for a virtual, its synthetic account)
    /// - `roots`: Array of optional MMR roots [current, pos0, pos1, pos2, pos3, pos4, pos5].
    ///   Replica sets Some(root) for positions they have, None for positions they don't.
    /// - `activate_replacement`: verify against the pending replacement and
    ///   activate it ("Replacement agreements")
    /// - `signatures`: the provider's signatures over `ReplicaSyncPayload`
    ///   (`version`, `genesis_hash`, `bucket_id`, the live or the pending `agreement_id`,
    ///   `roots`), satisfying the provider signature rule (`ProviderSignatures`). For a
    ///   virtual provider the member set is the live agreement's snapshot, or
    ///   with `activate_replacement` the pending successor's; the
    ///   accepted signers are stored in `last_sync_signers` and are the ones
    ///   slashed by a failed `challenge_replica` against it.
    #[pallet::weight(...)]
    pub fn confirm_replica_sync(
        origin: OriginFor<T>,
        bucket_id: BucketId,
        provider: T::AccountId,
        /// Array of optional MMR roots: [current, pos0, pos1, pos2, pos3, pos4, pos5]
        /// Signed inside `ReplicaSyncPayload` to attest which roots the replica has.
        roots: [Option<H256>; 7],
        activate_replacement: bool,
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
    /// Origin: the challenged provider; for a virtual provider, any member of
    /// the challenge's `virtual_liability.snapshot`.
    /// 
    /// Must provide the challenged chunk with Merkle proofs, show that the
    /// chunk index is past the leaf's last chunk (`ChunkOutOfRange`), claim
    /// the data was deleted (`Deleted`), or show the challenged state has been
    /// superseded by canonical. An invalid
    /// response is rejected (`InvalidProof`, `InvalidDeletionClaim`,
    /// `InvalidSupersededClaim`) and the challenge stays open until the
    /// deadline. A valid response decrements the provider's
    /// `pending_challenges`.
    /// For a `challenge_checkpoint` or `challenge_replica` challenge,
    /// `Superseded` is invalid (`InvalidSupersededClaim`). For a
    /// `challenge_replica` challenge, `Deleted` is invalid
    /// (`InvalidDeletionClaim`).
    /// 
    /// Parameters:
    /// - `challenge_id`: The challenge to respond to (deadline + index)
    /// - `response`: Proof, ChunkOutOfRange, Deleted, or Superseded response
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
    /// off-chain. Can only be called by the original challenger. Decrements
    /// the provider's `pending_challenges`, like any resolution. Emits
    /// `ChallengeCancelled`.
    #[pallet::weight(...)]
    pub fn cancel_challenge(
        origin: OriginFor<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
    ) -> DispatchResult;

    /// Slash a provider whose challenge expired without a response.
    ///
    /// Anyone may call this; it is free on success. Slashes the provider's
    /// whole stake to `T::OnSlash`. For a virtual provider, it slashes each
    /// member in `virtual_liability.liable` by the snapshot's
    /// `per_provider_stake`, also to `T::OnSlash`. The slash cascades to the
    /// other virtual providers a slashed account is a member of
    /// (virtual-provider extension, "Stake and Slashing"). Refunds the
    /// challenger's deposit and decrements the provider's
    /// `pending_challenges`. A provider may call it against itself to release
    /// its locks and unblock `deregister_provider`.
    /// If the bucket was deleted, it slashes nothing, refunds the deposit and
    /// emits `ChallengeCancelled`, as `cancel_challenge` does. This holds even
    /// if the deadline passed before the deletion. The missed slash is
    /// harmless: the bucket and its data are deleted, so nobody relies on the
    /// challenged data any more.
    /// Fails with `ChallengeNotExpired` while the provider may still respond
    /// (`anchor <= deadline`) and `ChallengeNotFound` once resolved.
    #[pallet::weight(...)]
    pub fn resolve_expired_challenge(
        origin: OriginFor<T>,
        challenge_id: ChallengeId<BlockNumberFor<T>>,
    ) -> DispatchResultWithPostInfo;
}

pub enum EndAction {
    /// Pay provider in full
    Pay,
    /// Burn locked payment entirely (to `T::OnBurn`).
    /// Additionally deducts `T::BurnPremium` (e.g., 10%) from caller's free balance, also to `T::OnBurn`.
    /// Fails if caller has insufficient funds for the premium.
    Burn,
}

pub enum RemovalReason {
    /// Agreement expired naturally
    Expired,
}

pub enum ChallengeResponse<T: Config> {
    /// Provide the chunk with proofs. `chunk_data` must have the length its
    /// index implies, else `InvalidProof`: the leaf version's `CHUNK_SIZE`,
    /// or `data_size - (chunk_count - 1) * CHUNK_SIZE` for the last chunk.
    Proof {
        chunk_data: BoundedVec<u8, T::MaxChunkSize>,
        mmr_proof: MmrProof,
        /// RFC 9162 inclusion path: sibling hashes from the chunk to the
        /// leaf's `data_root` ("Chunk Tree")
        chunk_proof: BoundedVec<H256, MaxChunkProofDepth>,
    },
    /// The challenged `chunk_index` is at or beyond the leaf's chunk count.
    /// The MMR proof carries the leaf, so the chain derives the count from
    /// `leaf.data_size` itself.
    ChunkOutOfRange {
        mmr_proof: MmrProof,
    },
    /// Data was deleted: the current snapshot's `start_seq` is past the
    /// challenged leaf. Only an admin's checkpoint raises `start_seq`
    /// (`checkpoint`), so the chain checks this from state; no evidence needed.
    /// Not valid for a `ChallengeKind::Replica` challenge: a replica is liable
    /// for exactly its `last_sync`. A challenge on a deleted bucket needs no
    /// response: `resolve_expired_challenge` slashes nothing. The provider's
    /// locks stay until the deadline; it may then call
    /// `resolve_expired_challenge` itself.
    Deleted,
    /// The current snapshot covers the challenged leaf. Valid iff all hold:
    ///   - the challenge is a `ChallengeKind::Offchain` challenge;
    ///   - the challenged commitment differs from the current snapshot's
    ///     commitment;
    ///   - `target.seq` is in the current snapshot's range
    ///     `[start_seq, start_seq + leaf_count)`.
    ///
    /// The pallet checks this when the provider responds. For an off-chain
    /// challenge, the snapshot can cover the leaf only through a checkpoint
    /// included after the challenge was created (`challenge_offchain`
    /// rejects a covered leaf). A commitment of the same length with a
    /// different root also counts as superseded. Otherwise answer with
    /// `Proof`, or with `Deleted` if the seq is below the snapshot's
    /// `start_seq`. A checkpoint challenge's commitment was canonical: a later
    /// checkpoint appends to it, and `Proof` still answers, or deletes it, and
    /// `Deleted` answers. A replica is liable for exactly its `last_sync`.
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
3. **Replica sync** — peaks, subtree, bulk MMR node and chunk-tree node
   fetch, sync status. Used by
   replica providers; read-only.

Every endpoint that returns a provider signature returns `provider_signatures`,
a list of `{ "signer": "<ss58>", "signature": "0x..." }`: one entry from a
physical provider; from a member of a virtual provider, at least the named
agreement's `k` entries, its own and those it collected from the other members
(the write path's collection round), ready to pass on-chain as
`ProviderSignatures`.

A node's agreements for a bucket are:

- the bucket's agreement of this node's provider, or of a virtual provider
  whose agreement snapshot contains this node;
- the pending successor of any agreement on the bucket, if the successor's
  snapshot contains this node. This covers a member that the replacement
  adds, which the old agreement's snapshot does not contain.

Endpoints that take an `agreement_id` (`/commit`, `/commitment`,
`/checkpoint-signature`, `/delete`) accept only the node's primary agreements
for the bucket that have not expired, a pending successor of a primary
agreement included. A pending successor counts as expired once its old
agreement has expired, because it can no longer activate. Any other
`agreement_id`, a replica agreement or an expired agreement included, fails
with `unknown_agreement`. A replica proves its state only through
`confirm_replica_sync` (`ReplicaSyncPayload`). The reads it uses to sync take
no `agreement_id` ("Replica Sync API"). For a virtual provider, the collection round uses the
snapshot and `k` of the named agreement.

A node stores a bucket while one of its agreements for the bucket exists,
primary or replica, or while a challenge it must answer is open (against its
provider, or with this node in the challenge's `virtual_liability.snapshot`).
A challenge on a deleted bucket needs no answer.
An agreement exists from creation until it settles, also after it expires.

### Authentication & RBAC

These endpoints require an `Authorization` header. The Rules list below gives
the role per endpoint.

- mutating Layer-0 endpoints: `PUT /node`, `POST /commit`, `POST /delete`;
- every Layer-0 **bucket-bound read**: `GET /commitment`,
  `GET /checkpoint-signature`, `GET /mmr_proof`, `GET /mmr_peaks`,
  `POST /fetch_mmr_nodes`, `POST /exists`, `GET /replica/historical_roots`,
  `GET /replica/sync_status`.

The provider node verifies an sr25519 signature locally. It resolves the
caller's role via a TTL-cached point read of the chain's
`BucketMembers(bucket_id, caller)` entry.

Three Reader-level exemptions, all resolved from the same cached lookup:

- **Public buckets** serve every bucket-bound read anonymously.
- **Replica-held buckets**: when one of the node's agreements for the bucket
  ("Off-Chain: Provider Node API") is a replica agreement, reads are served
  to anyone regardless of visibility — replicas serve everyone; visibility
  gates primaries only (design doc, "Bucket Visibility & Access"). Resolved
  from those agreements' `StorageAgreements` entries, cached and invalidated
  alongside the membership entries.
- **Operator self-auth**: a request signed by the node's *own* provider
  account passes any Reader check. Provider accounts are never bucket
  members, and the operator can read the node's disk anyway; this is what
  lets the provider's own tooling (e.g. the dashboard building a challenge
  response) fetch `/mmr_proof` on a private bucket. It never grants Writer
  or Admin.

**Hash-keyed reads are capability reads.** `GET /node`, `GET /read`,
`GET /chunk_proof` and `POST /fetch_nodes` are keyed by a blake2-256 hash or
data root and take a `bucket_id`. They serve only chunks and chunk-tree
nodes. The node answers 404 `bucket_not_found` for a bucket it does not store,
and serves the rest without authentication, because the node store might be a
single content-addressed namespace with cross-bucket dedup, so the node cannot
tell whether a hash belongs to the named bucket. Possession of the hash is the
capability. This might be reconsidered in future versions. Hashes of private data are unguessable, and every enumeration
surface that would reveal them is gated: the MMR via `/mmr_peaks` and
`/fetch_mmr_nodes`, and per-bucket listings. See the design doc's "Bucket
Visibility & Access" for the rule and its limits.

**MMR reads are bucket-bound.** MMR nodes and leaves are not in the node
store. The provider builds each bucket's MMR at `/commit` and keeps it per
bucket (`PUT /node` accepts only chunk-tree nodes). `POST /fetch_mmr_nodes`
looks a hash up in the named bucket's MMR only, so it gates at Reader like
every bucket-bound read.

> **⚠️ Under-specified — [#304](https://github.com/paritytech/web3-storage/issues/304).**
> This scheme grew organically across several crates and needs one source of
> truth: the wire format is currently defined twice (Rust `provider-auth`
> + TS `core`) and hand-synced; the provider also accepts a `<Bytes>`-wrapped
> form (what wallets / PAPI `signBytes` send) not documented below; and the
> signed message binds only method + bucket + timestamp — **no body or provider
> binding**, leaving a replay window (default 5 min skew). #304 tracks the
> canonical definition + the binding/replay fix. The `<genesis_hash_hex>` in
> the message below binds a header to one chain ("Chain binding" under
> `CommitmentPayload`); the current code does not include it yet.

```
Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<unix_timestamp>

Signed message: "web3storage:<genesis_hash_hex>:<METHOD>:<bucket_id>:<unix_timestamp>"
```

Rules:
- `<unix_timestamp>` must be within `--auth-max-skew` (default 5 minutes) of
  the provider's clock — otherwise `401 TimestampExpired`.
- Required role per endpoint: `Reader` for reads of access-controlled data,
  `Writer` for uploads/commits, `Admin` for delete and other destructive ops.
- Membership is cached: a chain event invalidates the affected bucket
  immediately; missing that, `--auth-cache-ttl` (default 30s) bounds the
  delay. If a refetch then fails, the cached entries are served for up to
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

Request (a chunk):
{
  "bucket_id": 1234,                   // u64
  "data": "<base64 encoded>"
}

Request (an internal node):
{
  "bucket_id": 1234,
  "children": ["0xleft...", "0xright..."]
}

A request carries exactly one of `data` and `children`, and no hash.

Note: HTTP API is used for simplicity and firewall-friendliness. Binary protocols
(e.g., libp2p streams) could be added later for efficiency. Base64 encoding adds
~33% overhead but keeps the API JSON-friendly. For high-throughput scenarios,
consider a binary endpoint or chunked transfer encoding.

Response (200 OK):
{ "hash": "0xabc..." }   // hash_chunk(data) or hash_chunk_node(left, right)

Response (400 Bad Request):
{ "error": "invalid_node" }               // both or neither of data and children
{ "error": "invalid_child_count", "expected": 2, "actual": 3 }
{ "error": "empty_chunk" }                // data has 0 bytes
{ "error": "chunk_too_large", "size": 300000, "max": 262144 }
{ "error": "children_missing", "missing": ["0xchild2..."] }
{ "error": "non_canonical_node" }         // accepts_internal is false

Response (507 Insufficient Storage):
{ "error": "quota_exceeded", "used": 1000000, "max": 1000000 }
```

The provider computes the node's hash with the chunk-tree prefixes ("Hash
Prefixes") and returns it. The client compares it with the hash it computed.
A different hash means the provider did not store the node the client built.

**Acceptance rules.** The provider stores a node only if the node is the root
of a canonical chunk tree ("Chunk Tree"). The provider checks this at upload
from the children's `NodeInfo`:

```rust
const CHUNK_SIZE: u64 = 256 * 1024; // leaf version 1

/// What the provider knows about a stored node without reading chunk data.
struct NodeInfo {
    /// For a chunk: its length. For an internal node: `l.bytes() + r.bytes()`
    /// of its children, stored with the internal node.
    bytes: u64,
}

impl NodeInfo {
    fn bytes(&self) -> u64 { self.bytes }
    /// Number of chunks under the node.
    fn chunks(&self) -> u64 { self.bytes().div_ceil(CHUNK_SIZE) }
    /// The last chunk under the node is shorter than `CHUNK_SIZE`.
    fn has_tail(&self) -> bool { self.bytes() % CHUNK_SIZE != 0 }
    /// A complete binary tree over full chunks.
    fn is_perfect(&self) -> bool { !self.has_tail() && self.chunks().is_power_of_two() }
}

/// `PUT /node` with `data` of `len` bytes.
fn accepts_chunk(len: u64) -> bool { 1 <= len && len <= CHUNK_SIZE }

/// `PUT /node` with `children`, both stored: `l` left, `r` right.
fn accepts_internal(l: &NodeInfo, r: &NodeInfo) -> bool {
    l.is_perfect() && r.bytes() <= l.bytes()
}
```

`accepts_internal` is the RFC 9162 split. For `n > 1` chunks, the split point
`k` is the largest power of two smaller than `n`. The left subtree is
therefore a perfect tree over `k` full chunks, and the short last chunk is on
the right. The right subtree covers the other `n - k <= k` chunks.

Conversely, suppose `l` is perfect over `2^a` chunks and
`r.bytes() <= l.bytes()`. Then `2^a < n <= 2^(a+1)`, so `k = 2^a` and `l` is
the left subtree. `r` is canonical because the provider accepted it under the
same rules (induction from `accepts_chunk`).

Example: five chunks, `c0` to `c3` of 256 KiB and `c4` of 100 KiB. The
canonical tree is `n01 = (c0, c1)`, `n23 = (c2, c3)`, `n0123 = (n01, n23)`,
`root = (n0123, c4)`. At `root`, `n0123` is perfect (four full chunks) and
`c4` has 100 KiB <= 1 MiB. The provider rejects these two internal nodes over
the same chunks with `non_canonical_node`:

- `(n012, n34)`, with `n012 = (n01, c2)` and `n34 = (c3, c4)` both accepted:
  `n012` has three chunks, so it is not perfect.
- `(c0, n1234)`, with `n1234 = (n12, n34)` accepted: `n1234` has 868 KiB,
  more than the 256 KiB of `c0`.

**Implementation requirement.** The provider must determine a node's `bytes`
without reading chunk data. One way: keep full and short chunks in separate
stores. An existence check then tells whether a chunk is full, and only the
short chunk (at most one per tree) needs its length read.

**Lookups by chunk index.** To find chunk `i` under a node (`/read`,
`/chunk_proof`, a challenge response), the provider descends from the node.
With `l` the left child, it goes left if `i < l.chunks()`, otherwise right
with `i - l.chunks()`. This reads O(log n) nodes and never collects the
tree's chunk hashes.

### Sync Protocol

Client discovers which nodes are missing before uploading.

```
Check Existence (batched)
─────────────────────────
POST /exists
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets)

Request:
{
  "bucket_id": 1234,
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

After uploading, the client names a commitment, `base`, and requests the
provider to append data roots to it.

```
Commit
──────
POST /commit
Authorization: Web3Storage <...>       # Writer or Admin

Request:
{
  "bucket_id": 1234,
  "agreement_id": 7,                            // agreement the commitment is under
  "leaf_version": 1,                            // MmrLeaf.leaf_version of the new leaves
  "base": { "mmr_root": "0xabc...", "start_seq": 0, "leaf_count": 5 },
                                                // or the empty MMR at the snapshot's start_seq (catch-up)
  "data_roots": ["0xroot1...", "0xroot2..."]    // appended in this order
}

Response (200 OK):
{
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
    // one signature over the one resulting CommitmentPayload{ version,
    // genesis_hash, bucket_id, agreement_id, commitment }, however many
    // roots the request appends; for a virtual provider one entry per
    // member that signed
}

Response (400 Bad Request):
{ "error": "unknown_agreement" }                // agreement_id not accepted ("Off-Chain: Provider Node API")
{ "error": "unknown_base" }                     // no stored peaks for base, base is not a prefix of the snapshot's MMR the provider stores, and base is not the empty MMR at the snapshot's start_seq
{ "error": "root_not_found", "missing": ["0xroot2..."] }
{ "error": "unsupported_leaf_version", "supported": [1] }

Response (409 Conflict):
{ "error": "base_superseded" }   // result's start_seq differs from the snapshot's, the snapshot is not a prefix of a result at least as long, or the result is shorter than the current snapshot and the provider stores the snapshot

Response (507 Insufficient Storage):
{ "error": "quota_exceeded", "used": 1000000, "max": 1000000 }
```

**Provider.** The provider handles a commit in these steps. For a bucket
without a snapshot, the current snapshot is the empty MMR at `start_seq` 0
(`checkpoint`).

1. It rejects a `leaf_version` it does not support
   (`unsupported_leaf_version`). `GET /info` lists the versions it supports.
2. It requires that it knows the peaks of `base` (`unknown_base`): it stores
   them, or `base` is a prefix of the current snapshot's MMR, which the
   provider stores and derives the peaks from, or `base` is the empty MMR at
   the current snapshot's `start_seq`, which needs no peaks. The empty base
   lets a provider without the bucket's history catch up: the requests
   re-append the snapshot's leaves in order, and the provider builds the full
   MMR from them.
3. It requires every `data_root` to be stored (`root_not_found`). This is the
   whole check, because every stored node is the root of a canonical tree
   ("Content-Addressed Storage") whose whole tree is stored (node pruning
   rule, "Storage Model"). Exception: for a pending successor's
   `agreement_id` of a virtual provider, a member first fetches the chunk
   tree of each missing root from the nodes of the old agreement's and the
   successor's snapshots, leavers included, by capability reads
   ("Authentication & RBAC"). It checks every node against its hash, stores
   each node once its children are stored, under the acceptance rules of
   `PUT /node`, and answers `root_not_found` only for a root it cannot fetch
   and store.
4. For each root, in request order, it appends
   `MmrLeaf { leaf_version, data_root, data_size }` to the MMR of `base`, with
   `data_size` the root's `bytes`. Root `i` gets leaf index
   `base.leaf_count + i`. The result is `commitment = { mmr_root, start_seq:
   base.start_seq, leaf_count: base.leaf_count + data_roots.len() }`.
5. It checks the result against the current snapshot (`base_superseded`). The
   result must have the snapshot's `start_seq`. If the result has at least the
   snapshot's `leaf_count`, the snapshot's MMR must be a prefix of the
   result's MMR. The provider checks this from the nodes of the result's MMR.
   A shorter result is useless as a commitment: its leaves are inside the
   snapshot's range, so `challenge_offchain` rejects a challenge on it
   (`LeafInSnapshot`) and `checkpoint` rejects it. A provider that stores an
   MMR with the snapshot's MMR as a prefix therefore rejects a shorter result
   (`base_superseded`). A provider that does not store the snapshot's MMR
   yet accepts a shorter result, so a catch-up can run over several commits,
   each with the previous result as `base`, for example one per run of leaves
   with the same `leaf_version`. A provider that already stores the snapshot
   rejects the intermediate commits of such a catch-up and signs the last
   one, whose `base` is a prefix of the snapshot's MMR (step 2). Once the
   provider stores the snapshot, the pruning rule lets it drop the leaves of
   forks it accepted before, and it may drop the peaks of every result
   shorter than the snapshot. A commit whose `base` is the result of a
   deletion not yet checkpointed (`/delete`) fails with `base_superseded`,
   because its `start_seq` differs from the snapshot's. The admin checkpoints
   the deletion before further commits.
6. It signs `CommitmentPayload` over `commitment` once, after all roots.

The provider keeps the peaks of every commitment it signed, except as step 5
allows. If `base` is not the latest commitment the provider signed, the new
commitment is a fork of that latest commitment. The protocol accepts forks.
A checkpoint makes one of them canonical (pruning rule, "Storage Model"). Two
writers that commit on the same base create two forks. The checkpoint contains
at most one of them. The other writer commits its roots again with the new
snapshot as `base`.

The response contains only signatures. A client has to compute the
commitment to verify them, so it cannot use a commitment without checking it.

**Writer steps.** A client that commits follows these steps:

1. It gets the peaks of `base`, from its own state or from `GET /mmr_peaks`.
   It checks `peaks.len() == popcount(base.leaf_count)` and
   `mmr_root(base.start_seq, base.leaf_count, peaks) == base.mmr_root`
   ("Hash Prefixes"). On a mismatch, it gets the peaks from another provider.
2. For each of its roots, in request order, it appends
   `MmrLeaf { leaf_version, data_root, data_size }`. It computes its own
   `commitment`.
3. It builds `CommitmentPayload` from its own `commitment` and the request's
   `agreement_id`, and verifies the response against that payload:
   - physical provider: the one signature, against the provider's
     `public_key`;
   - virtual provider: each signature against its signer's `public_key`. It
     drops the invalid ones; valid signatures of at least `k` distinct
     members of the named agreement's snapshot (`VirtualSnapshot`) must
     remain. For a pending successor's
     `agreement_id`, the snapshot is `PendingReplacement.virtual_snapshot`
     and `k` is the successor's. Signatures under a pending successor's id
     give no guarantee until it activates.

   If the signature is invalid, or fewer than `k` valid member signatures
   remain, the commit gave no guarantee; the client retries, for a virtual
   provider through another member (virtual-provider extension, "Write
   path").

   It also checks that the named agreement has not expired
   (`now < expires_at`; for a pending successor, the old agreement's
   `expires_at`): a commitment under an expired agreement is void. It also
   checks that the commitment has the on-chain snapshot's `start_seq` and at
   least its `leaf_count` (the empty MMR at `start_seq` 0 if the bucket has no
   snapshot); otherwise the commitment is void, and the writer commits its
   roots again with the snapshot as `base`. In a catch-up, the intermediate
   results are void by design: the client needs no signatures for them. It
   continues with a result as the next `base` once enough providers can build
   on it: a physical provider must have accepted it; for a virtual provider,
   at least `k` members must store the snapshot or have accepted the result.
   Only the last result, which reaches the snapshot's length, needs the
   signatures above.
4. On a mismatch, it reads `GET /commitment` to see the provider's state. It
   uses that response for debugging only.

The checkpoint submits the client's own `commitment`, never a value read from
a provider.

### Read

```
Read Chunks
───────────
GET /read?bucket_id=1&data_root=0x...&offset=0&length=2097152

Capability read: keyed by data root, unauthenticated; 404 for a bucket the
node does not store (see Authentication & RBAC).

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
  "version": "0.1.0",
  "leaf_versions": [1]   // MmrLeaf.leaf_version values /commit accepts
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
redemption; the node maps it to `BucketTarget`. `nonce` is the owner's current
`AgreementNonces` value, which the client reads from chain at the best block
(finalized can lag behind a just-redeemed quote). The node signs the terms as
requested, adding `provider` and `valid_until`.

The quote contains no price and no version. When redeeming, the client passes
the version at which it read the provider's terms as `expected_version`; the
chain reads price, replica sync price and stake from the provider and fails with
`ProviderVersionMismatch` if that version changed (see "Term Pinning").

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

The provider rejects requests outside its duration bounds, below its minimum
agreement size, beyond its capacity, or for a role it is not accepting.

Download Node
─────────────
GET /node?bucket_id=1&hash=0x...

Capability read: keyed by hash, unauthenticated. 404 for a bucket the node
does not store (see Authentication & RBAC).

Response (200 OK), a chunk:
{ "hash": "0xabc...", "data": "<base64 encoded>" }

Response (200 OK), an internal node:
{ "hash": "0xdef...", "children": ["0xleft...", "0xright..."] }

Response (404 Not Found):
{ "error": "not_found" }

Get Commitment (for challenge_offchain)
───────────────────────────────────────
GET /commitment?bucket_id=1234&agreement_id=7
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets)

Response:
{
  "bucket_id": 1234,
  "agreement_id": 7,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
}

Response (400 Bad Request):
{ "error": "unknown_agreement" }   // agreement_id not accepted ("Off-Chain: Provider Node API")

Note: The returned signatures cover a `CommitmentPayload` with the **real**
`leaf_count`, so they can back a bound `challenge_offchain` — the pallet
reconstructs the payload from the `commitment` and `agreement_id` the
challenger passes, plus the chain's own genesis hash, verifies the signatures
over it, then uses that `leaf_count` to bind the proof to the challenged
leaf in `respond_to_challenge`. The values returned here must therefore be passed
on-chain unchanged. The `/delete` response signs the same way and also
returns `leaf_count`. `/commit` signs the same payload over the commitment the
client computes (writer steps in "Commit"). `/checkpoint-signature` is
equivalent and is used for the on-chain `checkpoint` extrinsic.

Get Checkpoint Signature (for checkpoint extrinsic)
───────────────────────────────────────────────────
GET /checkpoint-signature?bucket_id=1234&agreement_id=7&prev_start_seq=0&prev_leaf_count=40
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets)
  // prev_*: the on-chain snapshot, optional

Response:
{
  "bucket_id": 1234,
  "agreement_id": 7,
  "mmr_root": "0xfed...",
  "start_seq": 0,
  "leaf_count": 42,
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }],
  "consistency_proof": { "prev_peaks": [...], "items": [...] }
    // present iff prev_start_seq == start_seq and 0 < prev_leaf_count <
    // leaf_count: proves the snapshot's MMR is a prefix of this one
    // (`checkpoint`, append; format: `MmrConsistencyProof`)
}

Response (400 Bad Request):
{ "error": "unknown_agreement" }
{ "error": "prev_out_of_range" }
  // the provider's state is behind the snapshot: prev_start_seq > start_seq,
  // or equal start_seq and prev_leaf_count > leaf_count

Response (409 Conflict):
{ "error": "not_prefix" }
  // same start_seq, but the snapshot is not a prefix of the provider's MMR
  // (the provider holds a fork)

Note: Signs the same payload as `/commitment`; kept as a separate endpoint
for the checkpoint workflow, where the signatures go into the
`checkpoint`/`extend_checkpoint` signature list (a virtual provider's entries
all-or-nothing, see `checkpoint`). The client puts a signature requested with
a pending successor's `agreement_id` in the signer's pending slot (`checkpoint`).
The client submits its own commitment (writer steps in "Commit"). It uses the
signatures and the consistency proof only if the returned `mmr_root`,
`start_seq` and `leaf_count` equal its commitment. It verifies both the
signatures and the proof against its commitment.

Get MMR Proof
─────────────
GET /mmr_proof?bucket_id=1234&mmr_root=0x...&leaf_index=5
Authorization: Web3Storage <...>       # Reader; the provider's own account
                                       # also passes (operator self-auth)

The proof is against the MMR with root `mmr_root`. The root binds
`start_seq` and `leaf_count`, so it names one commitment. `leaf_index` is the
leaf's position in that MMR: `seq - start_seq`.

Response:
{
  "leaf": { "leaf_version": 1, "data_root": "0x...", "data_size": 2097152 },
  "items": ["0x...", ...]
}

Response (404 Not Found):
{ "error": "unknown_commitment" }   // the node holds no MMR with this root

Get Chunk Proof
───────────────
GET /chunk_proof?bucket_id=1&data_root=0x...&chunk_index=3

Capability read: keyed by data root, unauthenticated; 404 for a bucket the
node does not store.

Response:
{
  "chunk_hash": "0xabc...",
  "proof": ["0xsibling1...", "0xsibling2..."]   // RFC 9162 inclusion path
}

Response (404 Not Found):
{ "error": "data_root_not_found" }

Response (400 Bad Request):
{ "error": "chunk_index_out_of_range" }   // chunk_index >= ceil(data_size / CHUNK_SIZE)

Delete Data (admin only)
────────────────────────
POST /delete
Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>
  // admin-signed header (same scheme as other mutating endpoints);
  // the signer must be an Admin member of the bucket

Request:
{
  "bucket_id": 1234,
  "agreement_id": 7,
  "keep_from_seq": 10
}

Response (200 OK):
{
  "mmr_root": "0xnew...",
  "start_seq": 15,
  "leaf_count": 5,
  "provider_signatures": [{ "signer": "5F...", "signature": "0x..." }]
}

Response (400 Bad Request):
{ "error": "invalid_signature" }
{ "error": "unknown_agreement" }

Response (400 Bad Request):
{ "error": "keep_from_seq_out_of_range" }  // outside [snapshot start_seq, snapshot end]

Response (403 Forbidden):
{ "error": "not_admin" }

Response (409 Conflict):
{ "error": "bucket_frozen" }

Note: In this example the on-chain snapshot covers `[0, 15)`. The provider
starts a new MMR at the snapshot's end (15) and appends the snapshot's leaves
`[keep_from_seq, 15)` to it again, with the new sequence numbers `[15, 20)`
(`checkpoint`, deletion). Appends the provider holds past the snapshot are an
off-chain branch under the pruning rule. The API keeps only a contiguous
suffix of the snapshot.

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
  "total_bytes": 42949672960
}

Note: Public observability endpoint (operators, the Prometheus/Grafana setup
in `docs/`). Aggregate totals only — a per-bucket breakdown would let anyone
enumerate the node's buckets, sidestepping the per-bucket read gates.
```

### Replica Sync Status

```
Get Historical Roots (informational)
────────────────────────────────────
GET /replica/historical_roots?bucket_id=1234
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets)

Response:
{
  "bucket_id": 1234,
  "current_root": "0xfed...",
  "historical_roots": ["", "", "", "", "", ""],
  "snapshot_block": 0
}

Note: Provider nodes do NOT track historical roots — only the chain does, in
`Bucket.historical_roots` (as commitments). This endpoint returns the local current MMR root
and placeholder entries for the historical positions; clients building
`confirm_replica_sync` calls should query the chain via runtime API.

Get Replica Sync Status
───────────────────────
GET /replica/sync_status?bucket_id=1234
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets)

Response:
{
  "bucket_id": 1234,
  "local_mmr_root": "0xfed...",
  "local_leaf_count": 42,
  "last_sync": null,   // the agreement's ReplicaSyncRecord { commitment, block }; null before the first confirmation
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

On a **private** bucket the primaries refuse `/mmr_peaks` and
`/fetch_mmr_nodes`. Both are Reader-gated, and a replica is not a member (the
design's primary gate). The sync coordinator therefore fetches MMR peaks,
nodes and leaves only from the bucket's *other replicas*. Replicas serve
everyone, which lets a public→private bucket keep seeding new replicas.
Chunk-tree nodes and chunks (`POST /fetch_nodes`) are capability reads, so
primaries remain a source for them. A born-private bucket has no honest source
for its MMR, by design.

**Why chain-first?**

The chain checkpoint is the source of truth. Fetching the root from a provider
would require trusting that provider. By getting the root from the chain first,
the replica can verify all fetched data against a trusted commitment.

```
Get MMR Peaks (given trusted root from chain)
─────────────────────────────────────────────
GET /mmr_peaks?bucket_id=1234&mmr_root=0xfed...
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets
                                       # and on buckets the serving node
                                       # holds as a replica)

Response:
{
  "bucket_id": 1234,
  "mmr_root": "0xfed...",
  "peaks": ["0xpeak1...", "0xpeak2...", ...]
}

Response (404 Not Found):
{ "error": "unknown_commitment" }   // the node holds no MMR with this root

Note: The replica already knows the trusted commitment (mmr_root, start_seq,
leaf_count) from the chain. It fetches peaks from a provider and verifies:
  peaks.len() == popcount(leaf_count), and
  mmr_root(start_seq, leaf_count, peaks) == trusted mmr_root ("Hash Prefixes").
If verification fails, it tries another provider. Once verified, it uses the
peaks to start top-down traversal.

Note: To check which nodes exist on a provider, use the existing POST /exists
endpoint from the Sync Protocol section above. The traversal walks MMR nodes
and leaves with POST /fetch_mmr_nodes. It walks chunk trees with
POST /fetch_nodes or GET /node.

Fetch MMR Nodes (batched, for sync)
───────────────────────────────────
POST /fetch_mmr_nodes
Authorization: Web3Storage <...>       # Reader (anonymous on public buckets
                                       # and on buckets the serving node
                                       # holds as a replica)

Request:
{
  "bucket_id": 1234,
  "hashes": ["0xpeak1...", "0x789...", ...]
}

Response:
{
  "nodes": [
    { "hash": "0xpeak1...", "children": ["0xleft...", "0xright..."] },  // MMR node
    { "hash": "0x789...", "leaf": { "leaf_version": 1, "data_root": "0x...", "data_size": 2097152 } }  // MMR leaf
  ]
}

Note: Serves only hashes in the named bucket's MMR. The response omits other
hashes.

Fetch Nodes (batched, for sync)
───────────────────────────────
POST /fetch_nodes

Capability read: keyed by hash, unauthenticated. 404 for a bucket the node
does not store (see Authentication & RBAC).

Request:
{
  "bucket_id": 1234,
  "hashes": ["0xdef...", "0x456...", ...]
}

Response:
{
  "nodes": [
    { "hash": "0xdef...", "children": ["0xleft...", "0xright..."] },  // internal node
    { "hash": "0x456...", "data": "<base64>" }                        // chunk
  ]
}

Note: Bulk fetch of nodes by hash. More efficient than individual GET /node
requests when syncing many nodes.
```

**Top-down sync algorithm:**

```
1. Query chain for bucket's current snapshot (mmr_root, start_seq, leaf_count)
   Also note historical_roots for fallback positions
2. Fetch mmr_peaks from any provider (on a private bucket, only other replicas)
3. Verify: peaks.len() == popcount(leaf_count) and
   mmr_root(start_seq, leaf_count, peaks) == trusted mmr_root from chain
   If mismatch, try another provider
4. Compare verified peaks with locally stored peaks
5. For each differing peak:
   a. Fetch subtree level by level (breadth-first)
   b. At each level, check which nodes exist locally
   c. Fetch missing nodes from any available provider:
      - MMR nodes and leaves with POST /fetch_mmr_nodes (on a private bucket,
        only from other replicas)
      - chunk-tree nodes and chunks with POST /fetch_nodes
   d. Verify each fetched node against expected_hash, using the prefix of the
      node's kind ("Hash Prefixes"). The node's place in the traversal gives
      its kind:
      - starting at a peak of height j (j from leaf_count): j levels of MMR
        nodes, the peak included, then MMR leaves
      - below an MMR leaf: its chunk tree, with root leaf.data_root. The
        tree's shape follows from the leaf's data_size
      Reject a mismatch
   e. Continue to children of newly fetched nodes
6. Once all nodes fetched and verified:
   a. Sign `ReplicaSyncPayload` — the `roots` matching on-chain
      `historical_roots`, the live or the pending `agreement_id` (a virtual
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
    /// `mmr_root(start_seq, leaf_count, peaks)` of the MMR containing all
    /// data_roots ("Hash Prefixes"). The empty MMR (`leaf_count == 0`) has
    /// `mmr_root(start_seq, 0, [])`.
    pub mmr_root: H256,
    /// Sequence number of the first leaf covered by this commitment
    pub start_seq: u64,
    /// Number of leaves covered by this commitment
    pub leaf_count: LeafCount,
}

// Canonical range: [start_seq, start_seq + leaf_count)

pub const MAX_LEAF_COUNT_BITS: u32 = 48;

/// Leaf count of an MMR. `Decode` rejects a value above
/// `2^MAX_LEAF_COUNT_BITS`, so a commitment with a larger count does not
/// decode. The cap bounds every MMR proof ("MMR Proof").
pub struct LeafCount(u64);
```

`ChunkLocation` is the companion *position* type (`Commitment` is a *range*):
the exact chunk a challenge targets.

```rust
pub struct ChunkLocation {
    /// Sequence number of the challenged leaf. A canonical sequence number is
    /// never reused, so it names the same leaf in every canonical commitment
    /// that contains it. Its position in a commitment's MMR is
    /// `seq - commitment.start_seq`.
    pub seq: u64,
    /// Index of the challenged chunk within the leaf's data: chunk `i` covers
    /// bytes `[i * CHUNK_SIZE, min((i + 1) * CHUNK_SIZE, data_size))`. An
    /// index at or beyond the chunk count is answered with
    /// `ChallengeResponse::ChunkOutOfRange`.
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
    /// `dev` today; adding `genesis_hash` and `agreement_id` below is a layout
    /// change and bumps it to 2 so v1 signatures cannot verify against the
    /// new payload.
    pub version: u8,
    /// Genesis hash of the chain the commitment is for. Not a call parameter
    /// and not in any HTTP response: the signer puts its chain's hash into
    /// the preimage and the chain reconstructs with its own
    /// (`frame_system::block_hash(0)`), so a signature made for another chain
    /// fails to verify.
    pub genesis_hash: H256,
    /// Reference to on-chain bucket. Mandatory — there is no anonymous /
    /// "best-effort" commitment mode in the current implementation.
    pub bucket_id: BucketId,
    /// Agreement this commitment is made under. A challenge validates the
    /// commitment only while that agreement is live; once it ends the
    /// commitment is void (see `StorageAgreement.agreement_id`). This is the
    /// sole replay bound: a signature is usable only against the exact agreement
    /// it names, and dies with that agreement — no separate time-based nonce is
    /// needed (see "Replay & commitment validity").
    pub agreement_id: AgreementId,
    /// MMR commitment being signed over
    pub commitment: Commitment,
}

/// What a replica signs for `confirm_replica_sync`. Bound to one agreement
/// exactly like `CommitmentPayload`.
pub struct ReplicaSyncPayload {
    /// `CURRENT_VERSION = 1`; bumps on any layout change, as for
    /// `CommitmentPayload`.
    pub version: u8,
    /// Chain binding, as in `CommitmentPayload`.
    pub genesis_hash: H256,
    /// The bucket the replica agreement is for.
    pub bucket_id: BucketId,
    /// The live agreement's id, or the pending replacement's to activate it.
    pub agreement_id: AgreementId,
    /// `[current, pos0, pos1, pos2, pos3, pos4, pos5]`; `Some` for the roots
    /// the replica has.
    pub roots: [Option<H256>; 7],
}
```

> **Chain binding.** Every signature the pallet verifies inside an extrinsic
> includes the chain's genesis hash in its preimage, so it is valid on one
> chain only. The ids it signs over are the same on every chain this pallet
> runs on: `bucket_id` and `agreement_id` are per-chain counters, and a
> virtual provider's account and `governance_nonce` derive from a per-chain id
> and a counter. A key used on two chains could otherwise have a signature
> from chain A replayed on chain B: a provider made liable for data it never
> stored there, or a governance change applied to the other chain's virtual
> provider. The rule covers `CommitmentPayload`, `ReplicaSyncPayload`, the
> quote context (`blake2_256(context | genesis_hash | SCALE(terms))`) and
> governance approvals (virtual-provider extension, "Membership Governance").
> Signed extrinsics are already chain-bound by `CheckGenesis`; this applies
> the same rule to the signatures inside them. The provider node's HTTP
> authentication header follows the same rule ("Authentication & RBAC").

### MMR Leaf

```rust
pub struct MmrLeaf {
    /// Leaf format version (`LEAF_VERSION = 1`). First field, so the leaf
    /// hash preimage is `0x02 ++ leaf_version ++ data_root ++ data_size`. The
    /// version defines the fields below, `CHUNK_SIZE` and the chunk tree shape.
    pub leaf_version: u8,
    /// Root of the chunk tree (see "Chunk Tree")
    pub data_root: H256,
    /// Byte length of the data under `data_root`. The chunk count is
    /// `ceil(data_size / CHUNK_SIZE)` and is never zero.
    pub data_size: u64,
}
// Sequence number is implicit: start_seq + leaf_position
```

The provider writes the request's `leaf_version` into each new leaf and
rejects a version it does not support (`/commit`). A verifier reads `leaf_version` first and checks the rest of the leaf
under that version: its fields, its `CHUNK_SIZE`, the chunk tree shape and the
hash prefixes. The pallet accepts every version it knows; a response whose
leaf has an unknown version fails with `InvalidProof`. A format change bumps
`LEAF_VERSION`; leaves of different versions share one MMR, so no root has to
be recomputed.

### Hash Prefixes

Every hash in the chunk tree and the MMR is `blake2_256` over a one-byte
prefix and the payload. Each hash kind has its own prefix, so the bytes of
one kind never verify as another kind. Integers in a preimage are `u64`
little-endian.

| Prefix | Preimage | Hashes | Function |
|---|---|---|---|
| `0x00` | `0x00 ++ chunk` | a chunk: leaf of the chunk tree | `hash_chunk` |
| `0x01` | `0x01 ++ left ++ right` | an internal node of the chunk tree | `hash_chunk_node` |
| `0x02` | `0x02 ++ leaf.encode()` | an `MmrLeaf`: leaf of the MMR (`leaf_version` is the first byte after the prefix) | `hash_mmr_leaf` |
| `0x03` | `0x03 ++ left ++ right` | an internal node of the MMR | `hash_mmr_node` |
| `0x04` | `0x04 ++ peak ++ bag_peaks(rest)` | the bag of two or more peaks, folded right to left | `bag_peaks` |
| `0x05` | `0x05 ++ start_seq ++ leaf_count ++ bag_peaks(peaks)` | `Commitment.mmr_root` | `mmr_root` |

`bag_peaks` of one peak is that peak. `bag_peaks` of no peaks is 32 zero
bytes. An MMR with `leaf_count` leaves has one peak per set bit of
`leaf_count`, highest first. A verifier that receives peaks rejects any other
number of them. A root is only ever computed from
`(start_seq, leaf_count, peaks)`. Every verifier takes the `Commitment` it
checks against, never a root and a count passed separately.

### Chunk Tree

The chunk tree is the binary Merkle tree of RFC 9162 (section 2.1.1) over the
chunks of one piece of data, with the hashes of "Hash Prefixes" in place of
SHA-256. The tree over one chunk is `hash_chunk(chunk)`. The tree over `n > 1`
chunks is `hash_chunk_node(tree(chunks[0..k]), tree(chunks[k..n]))`, with `k`
the largest power of two smaller than `n`. The tree's root is the leaf's
`data_root`.

Every chunk has `CHUNK_SIZE` bytes (256 KiB in leaf version 1) except the
last, which has 1 to `CHUNK_SIZE` bytes. So `n = ceil(data_size / CHUNK_SIZE)`
and `n >= 1`. Empty data has no chunks. The client does not upload it, and
the provider rejects an empty chunk at `PUT /node` (`empty_chunk`). A tree
with this shape and these chunk sizes is **canonical**. Each piece of data has
exactly one canonical tree, so exactly one `data_root`. The provider checks
the whole shape at upload, one node at a time (`accepts_internal`,
"Content-Addressed Storage").

Pairing nodes left to right at each level and moving an unpaired last node up
unchanged builds the same tree. Reference implementation:
`storage_primitives::merkle` (`merkle_root`, `build_merkle_proofs`,
`verify_merkle_proof`).

A chunk proof is the RFC 9162 inclusion proof (section 2.1.3.1): the sibling
hashes from the chunk to the `data_root`, and nothing else. The verifier runs
the algorithm of section 2.1.3.2. The RFC's leaves are the chunks, so its
`leaf_index` is `chunk_index` and its `tree_size` is the chunk count `n`. It
derives every left/right direction from those two values.
The prover therefore cannot choose the directions, and the proof is bound to
the chunk count that `data_size` implies. A proof against a tree of another
size fails.

### MMR Proof

All proofs are extrinsic arguments, so every list is bounded. The bounds
derive from `MAX_LEAF_COUNT_BITS` (`LeafCount`) and from the `u64` chunk
index.

```rust
/// Maximum number of sibling hashes in a chunk proof: chunk indices are `u64`.
pub type MaxChunkProofDepth = ConstU32<64>;
/// Maximum number of peaks: one per set bit of a `LeafCount`.
pub type MaxMmrPeaks = ConstU32<MAX_LEAF_COUNT_BITS>;
/// Maximum number of items in an `MmrProof` or an `MmrConsistencyProof`.
pub type MaxMmrProofItems = ConstU32<{ MAX_LEAF_COUNT_BITS + 1 }>;

pub struct MmrProof {
    /// The leaf being proven.
    pub leaf: MmrLeaf,
    /// The hashes that connect the leaf to the root, in this order: the
    /// roots of the peaks left of the leaf's peak, the leaf's siblings up to
    /// its peak (bottom up), and one hash that bags the peaks right of it, if
    /// any. The verifier derives their number and positions from the
    /// challenge (below).
    pub items: BoundedVec<H256, MaxMmrProofItems>,
}

/// Proves that the MMR of `prev` is a prefix of the MMR of `new` (same
/// `start_seq`). Hashes only. Their number and order follow from the two
/// leaf counts (below).
pub struct MmrConsistencyProof {
    /// Peaks of the earlier MMR, highest first.
    pub prev_peaks: BoundedVec<H256, MaxMmrPeaks>,
    /// The right siblings that grow the lowest earlier peak into the new peak
    /// that contains the earlier MMR's last leaf, lowest level first, then
    /// one hash that bags the new peaks right of that peak, if any (below).
    pub items: BoundedVec<H256, MaxMmrProofItems>,
}
```

A peak of height `j` covers `2^j` leaves. The peaks of an MMR with `n` leaves
are the set bits of `n`, highest first ("Hash Prefixes"). Proofs carry no node
positions and no sizes. The verifier derives both from counts it already has.

**`MmrProof`.** The inputs are
`leaf_index = Challenge.target.seq - Challenge.commitment.start_seq` and
`n = Challenge.commitment.leaf_count`. The verifier never takes these values
from the proof. It requires `leaf_index < n` and derives:

- `h = ilog2(leaf_index XOR n)`: the height of the leaf's peak.
- The peaks left of the leaf's peak: one for each set bit of `n` above `h`.
- `offset = leaf_index & ((1 << h) - 1)`: the leaf's position in its peak. At
  level `j` (`0 <= j < h`, level 0 is the leaf), bit `j` of `offset` gives the
  side:
  - 0: the current node is a left child, and the sibling is on the right;
  - 1: the current node is a right child, and the sibling is on the left.
- The peaks right of the leaf's peak: one for each set bit of `n` below `h`.

Items, in this order:
1. the root of each peak left of the leaf's peak, left to right;
2. the sibling at each level from the leaf up to its peak, bottom up;
3. if any peaks are right of the leaf's peak, one hash: `bag_peaks` of them.

The verifier rejects any other number of items. It then:

1. hashes the leaf (`hash_mmr_leaf`);
2. climbs to the peak with `hash_mmr_node`;
3. bags the left peaks, the leaf's peak and the right bag;
4. compares `mmr_root(start_seq, n, ..)` with `Challenge.commitment.mmr_root`.

With `n <= 2^48`, the left peaks plus the siblings are at most
`MAX_LEAF_COUNT_BITS` items, plus one bag.

Example: `n = 5` (`0b101`), leaf 2.

- `2 XOR 5 = 0b111`, so `h = 2`. The leaf's peak covers leaves 0 to 3.
- `n` has no set bit above 2, so there are no left peaks.
- `offset = 0b10`. At level 0 the node is a left child, and the sibling is
  leaf 3. At level 1 the node is a right child, and the sibling is the node
  over leaves 0 and 1.
- Bit 0 of `n` is set, so one peak (leaf 4) is right of the leaf's peak.

Items: `[hash_mmr_leaf(leaf 3),
hash_mmr_node(hash_mmr_leaf(leaf 0), hash_mmr_leaf(leaf 1)), hash_mmr_leaf(leaf 4)]`.

**`MmrConsistencyProof`.** The inputs are the two commitments `prev` and `new`
(`checkpoint`, append). Both have the same `start_seq`, and
`0 < m = prev.leaf_count < n = new.leaf_count`. The verifier:

1. Requires `prev_peaks.len() == popcount(m)` and
   `mmr_root(prev.start_seq, m, prev_peaks) == prev.mmr_root`.
2. Derives `H = ilog2((m - 1) XOR n)`, the height of the new peak that
   contains leaf `m - 1`. The previous peaks for set bits of `m` above `H` are
   also the new peaks left of that new peak.
3. Grows the lowest previous peak (height `t = trailing_zeros(m)`) into that
   new peak. At each level `j` (`t <= j < H`):
   - if bit `j` of `m - 1` is 1, the sibling is on the left and is the
     previous peak of height `j`;
   - if bit `j` of `m - 1` is 0, the sibling is on the right and is the next
     item.
4. If `n` has set bits below `H`, takes one more item: `bag_peaks` of the new
   peaks right of the grown peak.
5. Rejects any other number of items. Bags the peaks from steps 2 to 4 and
   compares `mmr_root(new.start_seq, n, ..)` with `new.mmr_root`.

The items are the right siblings of step 3, lowest level first, then the bag
of step 4. That is at most `MAX_LEAF_COUNT_BITS + 1` items.

Reference implementation: a wrapper around
`polkadot-ckb-merkle-mountain-range`. Its `StorageMerge::merge(left, right)`
hashes `0x03 ++ left ++ right`. The library calls
`merge_peaks(right_bag, left_peak)`, and `StorageMerge::merge_peaks` hashes
`0x04 ++ left_peak ++ right_bag`. The wrapper derives the library's node
positions and MMR size from the counts above, and computes roots with
`mmr_root`. For `MmrConsistencyProof`, the wrapper does not use the library's
`AncestryProof`. That proof carries node positions and a size the prover
chooses, and its root binds no leaf count.

---

## Challenge Protocol

### Timeline

```
1. Challenger initiates challenge on-chain
   └─ Provides: signed commitment, seq, chunk_index
   └─ `T::ChallengeDeposit` is held from the challenger (Config)
   └─ Tier determined by is_authorized(challenger, bucket, provider):
      authorized vs. general public

2. Challenge window opens (`ChallengeTimeout`, in anchor blocks)
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

3b. Provider responds with `Deleted` (not valid for a replica challenge)
    └─ The current snapshot's start_seq is past the challenged seq
    └─ Challenge rejected (data was legitimately deleted)
    └─ Treated as a valid response: provider's fee reimbursed as in 3a,
       remainder returned to challenger; stake untouched

3c. Provider responds with `Superseded` (off-chain challenges only)
    └─ A checkpoint included after the challenge was created covers the
       challenged seq, and its commitment differs from the challenged one.
       The leaf at that seq may be unchanged: the snapshot voids the
       provider's other commitments for every leaf it covers, so no
       divergence proof is needed
    └─ Treated as a valid response: provider's fee reimbursed as in 3a,
       remainder returned to challenger; stake untouched

3d. Provider responds with `ChunkOutOfRange`
    └─ The challenged chunk index is at or past the proven leaf's chunk count
    └─ Treated as a valid response: provider's fee reimbursed as in 3a,
       remainder returned to challenger; stake untouched

3e. Provider responds with an invalid proof or claim
    └─ Response rejected; the provider pays the transaction fee
    └─ Challenge stays open; the provider may answer again before the deadline

3f. Provider fails to respond by the deadline
    └─ Anyone submits `resolve_expired_challenge` (free on success; the
       challenger has the deposit as a reason to)
    └─ Provider's contract stake fully slashed (for a virtual provider,
       `per_provider_stake` per liable member)
    └─ Challenger's deposit refunded in full; it pays its own fee for
       creating the challenge, and resolving is free. No reward from the
       slash (no profit motive for forcing slashes), regardless of tier
    └─ Clear on-chain evidence of provider fault
    └─ If the bucket was deleted: no slash, the deposit is refunded in full
       (`ChallengeCancelled`)
```

**Why this cost model?**
- **Strangers can't drain a provider (anti-DDoS)**: A public challenge leaves an honest provider whole in money terms (fee fully reimbursed, stake untouched). If strangers got the split instead, a crowd could each pay little while collectively draining the provider; full-cost-per-stranger makes the attackers' cost scale with the damage. A stranger can still impose on-chain work and a reputation hit, but cannot extract value or grind down stake.
- **A provider can't serve everyone equally**: so a stranger being made to wait (e.g. under a lot of load) isn't evidence of fault—unlike a paying counterparty's unanswered request.
- **Owners get leverage, not cheap recovery**: the split lets a counterparty pressure the provider into serving, but with the challenger's share floored at 50% of a high on-chain cost, it stays a last-resort tool—recovering data at scale this way is unreasonably expensive even for the owner.
- **Monetary exposure is bounded to chosen counterparties**: a provider is made to bear cost only for accounts it accepted agreements with (or the admin added)—it controls that risk by vetting whom it signs with.
- **Off-chain resolution preferred**: answering on-chain means posting the data as a transaction—far costlier than serving the same bytes off-chain (the bandwidth is spent either way)—plus in-window hassle and reputation damage, even when the fee is reimbursed. So the provider serves directly.

> **Note on the deposit/fee mechanic.** The deposit is sized to the *transaction cost* of the provider's response, not a slice of stake. A simple implementation: the provider pays the response fee from its account when it submits the proof, and the challenge-resolution logic refunds that fee out of the locked deposit (in full for public challengers, or the table fraction for authorized ones), returning any remainder to the challenger. No stake movement occurs on a valid response—stake is only ever touched by the slash in 3f.

### Verification

```rust
/// Judged in one step. A valid response settles the deposit; an invalid
/// one is rejected with the returned error and the challenge stays open, so
/// the provider may correct and resend before the deadline. Only a timeout
/// slashes (see `resolve_expired_challenge`).
fn verify_challenge_response(
    challenge: &Challenge,
    response: &ChallengeResponse,
    bucket: Option<&Bucket>, // None: the bucket was deleted
) -> Result<(), Error> {
    let challenged_seq = challenge.target.seq;
    // Position in the committed MMR; creation checked
    // start_seq <= seq < start_seq + leaf_count.
    let leaf_index = challenged_seq
        .checked_sub(challenge.commitment.start_seq)
        .ok_or(Error::InvalidProof)?;
    match response {
        ChallengeResponse::Proof { chunk_data, mmr_proof, chunk_proof } => {
            // The chunk must sit in the leaf and the leaf in the committed MMR.
            // The chunk count comes from the proven leaf, not from the prover.
            let leaf = &mmr_proof.leaf;
            let chunk_count = chunk_count(leaf); // ceil(data_size / CHUNK_SIZE of leaf_version)
            let chunk_hash = hash_chunk(chunk_data); // blake2_256(0x00 ++ chunk_data)
            // RFC 9162 section 2.1.3.2 with (chunk_index, chunk_count). False if
            // the path does not fit them or does not lead to leaf.data_root.
            let chunk_ok = verify_merkle_proof(
                chunk_hash,
                challenge.target.chunk_index,
                chunk_count,
                chunk_proof,
                &leaf.data_root,
            );
            // Bound to the challenged leaf: the leaf position and the MMR shape
            // come from (leaf_index, commitment.leaf_count), not from the
            // proof, so a proof for a different leaf of the same MMR does not verify.
            let mmr_ok = verify_mmr_proof(mmr_proof, leaf_index, &challenge.commitment);
            // The chunk must have the length its index implies
            // (ChallengeResponse::Proof). A response with a tree of the right
            // shape over shorter chunks is rejected. len_ok is only evaluated
            // once chunk_ok holds, so the index is below chunk_count.
            let len_ok = || chunk_data.len() as u64 == chunk_len(leaf, challenge.target.chunk_index);
            if chunk_ok && mmr_ok && len_ok() { Ok(()) } else { Err(Error::InvalidProof) }
        }

        ChallengeResponse::ChunkOutOfRange { mmr_proof } => {
            // The challenged leaf must be in the committed MMR and have no chunk at
            // the index. Bound to leaf_index like the Proof arm, so the
            // prover cannot pick a shorter leaf to claim the index is out of range.
            let leaf = &mmr_proof.leaf;
            let mmr_ok = verify_mmr_proof(mmr_proof, leaf_index, &challenge.commitment);
            let out_of_range = challenge.target.chunk_index >= chunk_count(leaf);
            if mmr_ok && out_of_range { Ok(()) } else { Err(Error::InvalidProof) }
        }

        ChallengeResponse::Deleted => {
            // A checkpoint raised start_seq past the challenged leaf: only an
            // admin can do that, and start_seq never decreases. A replica is
            // liable for its last_sync, so a deletion is no defense for it. On
            // a deleted bucket no response is needed: resolve_expired_challenge
            // slashes nothing.
            match bucket.and_then(|b| b.snapshot.as_ref()) {
                Some(snapshot)
                    if challenge.kind != ChallengeKind::Replica
                        && challenged_seq < snapshot.commitment.start_seq => Ok(()),
                _ => Err(Error::InvalidDeletionClaim),
            }
        }

        ChallengeResponse::Superseded => {
            // Only an off-chain commitment can be a fork that no checkpoint made
            // canonical. A checkpoint challenge's commitment was canonical: later
            // checkpoints append to it (Proof still answers) or delete it
            // (Deleted answers). A replica is liable for exactly its last_sync.
            if challenge.kind != ChallengeKind::Offchain {
                return Err(Error::InvalidSupersededClaim);
            }
            // Provider can defend if the challenged commitment was replaced by a
            // newer canonical snapshot that still covers the challenged leaf:
            // an earlier state of the current MMR, or an off-chain fork that no
            // checkpoint made canonical.
            //
            // Deleted covers data before the snapshot's start_seq; Superseded
            // covers data inside the canonical range.
            //
            // Provider IS liable when the challenged commitment is still canonical (the
            // data is live, only a Proof defends it) or when challenged_seq lies
            // beyond the snapshot's end (start_seq + leaf_count): they signed
            // something canonical never covered.
            let Some(snapshot) = bucket.and_then(|b| b.snapshot.as_ref()) else {
                // Nothing canonical to lean on: the claim is unsupported.
                return Err(Error::InvalidSupersededClaim);
            };
            if challenge.commitment != snapshot.commitment
                && snapshot.commitment.start_seq <= challenged_seq
                && challenged_seq - snapshot.commitment.start_seq < snapshot.commitment.leaf_count
            {
                Ok(())
            } else {
                Err(Error::InvalidSupersededClaim)
            }
        }
    }
}
```

### Transaction extension for raced calls

Two calls can be submitted by several parties for one outcome, where only one
can succeed: a challenge response — a provider racing its challenger's
`cancel_challenge`, or two members of a virtual provider — and a replica sync
confirmation, which any snapshotted member of a virtual replica may submit for
the same round. A response carries a chunk of up to 256 KiB, so its fee is
dominated by length. Today both would be included and both charged:
transaction validity does not consult pallet state, and a dispatch that fails
still pays. A runtime `TransactionExtension` (the `CheckNonce` pattern) handles
both:

- `respond_to_challenge { challenge_id, .. }`: `validate` fails with `Stale` if
  the challenge does not exist and with `BadSigner` if the signer is not an
  eligible responder — the challenged provider, or for a virtual provider a
  member of the challenge's `virtual_liability.snapshot`. Otherwise
  `provides: [challenge_id]`.
- `confirm_replica_sync { bucket_id, provider, activate_replacement, roots, .. }`:
  `Stale` if there is no replica agreement, if `activate_replacement` is set
  and none is pending, if `min_sync_interval` has not elapsed since
  `last_sync.block` (not applied when activating a pending replacement), or if
  the sync would not advance — no root in `roots`
  matches a canonical or historical commitment that advances over
  `last_sync` (or equals it when activating; "advances" as in `confirm_replica_sync`). `BadSigner` if the signer
  is not the provider named in `provider`, or for a virtual provider not a
  member of the selected snapshot. Otherwise
  `provides: [(bucket_id, provider)]`: one sync per agreement in
  flight; a further advance is submitted once the previous one is included.

A transaction that fails `validate` is rejected by the first pool that sees it
and never gossiped further. The pool keeps at most one ready transaction per
tag, so a second response or sync is rejected at import (a higher tip replaces
the pending one instead). Once one is included, every pool extracts its tags
(validating at the parent block, where it is still valid) and prunes the
others. A straggler that reaches the block builder anyway fails `validate` at
the new block — the challenge is gone, the sync no longer advances — and is
dropped uncharged.

The loser of a race pays nothing and takes no block space. A transaction that
passes `validate` but fails at dispatch — a squatter taking the tag with
garbage; for a sync only the provider or a snapshotted member can take the tag
— is included and pays the full fee. Other calls pass through the extension unchanged.

---

## Open Questions

- **Who pays the sync confirmation fee.** `confirm_replica_sync` is submitted
  by one member of a virtual replica while `sync_price` is split equally.
  Direction: on success the pallet refunds the submitter's fee from the
  agreement's `sync_balance` before splitting `sync_price`; if the balance
  cannot cover it, the submitter pays. Open: a fixed `SyncFeeRefund` constant
  or the actual fee via `pallet_transaction_payment::compute_fee`. (`Pays::No`
  is out: every bound on free syncs is owner-controlled.) Later: route part of
  the fee to the primaries that served the sync.
- **Early termination of primaries by the admin.** Primaries are
  admin-controlled for write coordination, so the admin must keep control over
  who accepts writes; without early termination a hostile, misbehaving
  or slashed primary keeps its slot until expiry. `MaxPrimarySlots` may
  make that rare enough to ignore. If it is added, it should be
  exceptional: burn, not pay, and only when the bucket is at capacity. Not
  in the design today; the pallet still has `RemovalReason::AdminTerminated`
  and an early-termination path in `end_agreement`, to be removed.
