// SPDX-License-Identifier: Apache-2.0

//! # Storage Provider Pallet
//!
//! A pallet for scalable Web3 storage with game-theoretic guarantees.
//!
//! ## Overview
//!
//! This pallet implements a bucket-based storage system where:
//! - Providers register with stake and offer storage services
//! - Clients create buckets to organize their data
//! - Storage agreements bind providers to store data for agreed durations
//! - Challenges enforce accountability through slashing
//!
//! The chain acts as a credible threat, not the hot path. Normal operations
//! (reads, writes, storage) happen off-chain between clients and providers.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

pub mod impls;
pub mod runtime_api;
pub mod weights;
pub use weights::WeightInfo;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[frame_support::pallet]
// Several extrinsics in this pallet legitimately take more than 7 args (e.g.
// `checkpoint` now takes 7 explicit + a replay-protection nonce). The
// macro-generated wrapper functions exceed clippy's `too_many_arguments`
// threshold even when the originals have `#[allow(...)]`, so allow at the
// module level.
#[allow(clippy::too_many_arguments)]
pub mod pallet {
    use crate::weights::WeightInfo;
    use alloc::vec;
    use alloc::vec::Vec;
    use frame_support::{
        pallet_prelude::*,
        traits::{BalanceStatus, Currency, ExistenceRequirement, ReservableCurrency},
        CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
    };
    /// The parachain block height. Re-exported so dependent pallets get the
    /// same pair of names from one place; hooks-only — see [`BlockNumberFor`].
    pub use frame_system::pallet_prelude::BlockNumberFor as SystemBlockNumberFor;
    use frame_system::pallet_prelude::*;
    use sp_core::H256;
    use sp_runtime::traits::{Bounded, CheckedAdd, Saturating, Zero};
    #[cfg(feature = "try-runtime")]
    use sp_runtime::TryRuntimeError;
    use storage_primitives::{
        BucketId, BucketSnapshot, ChallengeId, ChallengerStatRecord, ChunkLocation, Commitment,
        CommitmentPayload, EndAction, MerkleProof, MmrProof, ProviderRole, RemovalReason,
        ReplayWindow, ReplicaSyncRecord, Role, SlashReason, Visibility,
    };

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    /// The anchor clock ([`Config::BlockNumberProvider`], relay chain in
    /// production) that every duration, deadline and expiry in this pallet is
    /// measured against. Deliberately shadows the `frame_system` name,
    /// treasury-style: the habitual spelling yields the correct clock, and the
    /// parachain height is only reachable as [`SystemBlockNumberFor`] (hooks
    /// only). The `Config` bound pins the two equal.
    pub type BlockNumberFor<T> = <<T as Config>::BlockNumberProvider as
        sp_runtime::traits::BlockNumberProvider>::BlockNumber;

    /// Provider-signed agreement quote bound to this pallet's account, balance,
    /// and block-number types.
    pub type AgreementTermsOf<T> = storage_primitives::AgreementTerms<
        <T as frame_system::Config>::AccountId,
        BalanceOf<T>,
        BlockNumberFor<T>,
    >;

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    /// Maximum deadline keys the slash sweep probes per block. Relay block
    /// numbers can jump by more than one per parachain block, so the sweep
    /// covers a range; this caps the probing and the remainder carries over via
    /// [`LastSweptChallengeBlock`]. Slashing is bounded separately by
    /// [`MAX_SWEEP_SLASH_BUDGET`].
    pub(crate) const MAX_SWEEP_SPAN: u32 = 32;

    /// Maximum challenges the slash sweep slashes per block, across all deadline
    /// keys it touches. Decoupled from [`Config::MaxChallengesPerDeadline`] (up
    /// to 1000) because slashing that many in one block would consume the whole
    /// block's PoV (~5 KB each). A fully loaded deadline instead drains over
    /// several blocks via the [`LastSweptChallengeBlock`] carry-over. The
    /// effective budget is `min(MaxChallengesPerDeadline, MAX_SWEEP_SLASH_BUDGET)`,
    /// so runtimes with a smaller per-deadline cap (e.g. tests) are unaffected.
    pub(crate) const MAX_SWEEP_SLASH_BUDGET: u32 = 100;

    #[pallet::hooks]
    impl<T: Config> Hooks<SystemBlockNumberFor<T>> for Pallet<T> {
        /// Slash providers whose challenges expired unanswered.
        ///
        /// Deadlines are relay-chain blocks ([`Config::BlockNumberProvider`]),
        /// which can jump by more than one per parachain block, so this drains a
        /// *range* of deadline keys, tracking progress in
        /// [`LastSweptChallengeBlock`] rather than probing the single key `n`.
        ///
        /// - **Which keys are final.** In `on_initialize` the validation-data
        ///   inherent has not run, so [`Pallet::current_anchor_block`] is the relay
        ///   parent `p` of the *previous* parachain block. A challenge with
        ///   deadline `d` stays respondable while some block has relay parent
        ///   `<= d`; every future block has relay parent `>= p`; so keys `< p`
        ///   are unrespondable and draining them cannot race a valid response.
        ///   Cost: a one-block lag — the slash lands the block after `p` passes
        ///   `d`. Escape hatches are unaffected; they gate on the
        ///   [`PendingChallenges`] counters, not on the sweep.
        /// - **Budget.** [`MAX_SWEEP_SPAN`] caps keys probed per block;
        ///   [`MAX_SWEEP_SLASH_BUDGET`] caps slashes per block so one maturing
        ///   deadline cannot eat the block's PoV. On exhaustion the cursor parks
        ///   just below the partly drained key; the rest carries over.
        /// - **Why `on_initialize`.** Work done is returned as weight instead of
        ///   pre-reserved, which `on_finalize` cannot do.
        ///
        /// The algorithm lives in [`Pallet::sweep_expired_challenges`] (and its
        /// `challenge_sweep_range` / `slash_expired_at` helpers) so the range
        /// resolution and the per-key drain read as separate, testable steps.
        fn on_initialize(_do_not_use_local_block_number: SystemBlockNumberFor<T>) -> Weight {
            Self::sweep_expired_challenges()
        }

        fn integrity_test() {
            // The re-register replay defense relies on RequestTimeout being strictly
            // shorter than DeregisterAnnouncementPeriod: a quote signed at block S
            // expires at S+RequestTimeout, which is before the provider can complete
            // deregistration and re-register (requiring DeregisterAnnouncementPeriod
            // more blocks), so an old quote cannot be replayed against the new
            // incarnation.
            // At the same time, the deregistration announcement window must be
            // strictly longer than the challenge response timeout, so any
            // challenge created up to the announcement block matures (and the
            // provider stays slashable) strictly before the provider can
            // complete deregistration.
            assert!(
                T::RequestTimeout::get() < T::DeregisterAnnouncementPeriod::get()
                    && T::DeregisterAnnouncementPeriod::get() > T::ChallengeTimeout::get(),
                "RequestTimeout must be less than DeregisterAnnouncementPeriod \
                to close the re-register replay window, and \
                DeregisterAnnouncementPeriod must be > ChallengeTimeout so a \
                challenge created at the announcement block matures while the \
                provider is still slashable"
            );
            assert!(
                T::AnchorBlockTimeMillis::get() > 0,
                "AnchorBlockTimeMillis must be non-zero; off-chain consumers \
                convert anchor-denominated durations to wall-clock time with it"
            );
        }

        #[cfg(feature = "try-runtime")]
        fn try_state(_block: SystemBlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

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
        #[pallet::constant]
        type MinProviderStake: Get<BalanceOf<Self>>;

        /// Maximum chunk size for challenge responses (e.g., 256 KiB).
        #[pallet::constant]
        type MaxChunkSize: Get<u32>;

        /// Timeout for challenge response (e.g., ~48 hours in relay chain
        /// blocks).
        #[pallet::constant]
        type ChallengeTimeout: Get<BlockNumberFor<Self>>;

        /// Deposit required to open a challenge. Reserved from the challenger
        /// on `challenge_*` and refunded (minus a response-time-proportional
        /// cost share) when the provider successfully defends, or returned
        /// in full alongside a 10% slash reward when the provider is
        /// slashed. Sets the floor on challenge spam economics — too low
        /// and griefing is free; too high and legitimate challenges become
        /// unaffordable.
        // DRIFT-005: this config item is missing from the design doc's Config
        // sketch.
        #[pallet::constant]
        type ChallengeDeposit: Get<BalanceOf<Self>>;

        /// Settlement window (in relay chain blocks) after agreement expiry
        /// for owner to call end_agreement.
        #[pallet::constant]
        type SettlementTimeout: Get<BlockNumberFor<Self>>;

        /// Maximum duration (in relay chain blocks) for agreement requests
        /// before expiry.
        #[pallet::constant]
        type RequestTimeout: Get<BlockNumberFor<Self>>;

        /// Maximum number of buckets a single account can be a member of.
        #[pallet::constant]
        type MaxBucketsPerMember: Get<u32>;

        /// Minimum number of relay chain blocks between announcing a
        /// deregistration and being allowed to complete it. Must be
        /// `> ChallengeTimeout` so any
        /// challenge against this provider that was created up to the
        /// announcement block matures while the provider is still slashable.
        #[pallet::constant]
        type DeregisterAnnouncementPeriod: Get<BlockNumberFor<Self>>;

        /// Maximum number of challenges that may share a single deadline
        /// (relay chain block), and the per-block slash budget of the
        /// `on_initialize` timeout sweep.
        ///
        /// Bounds the per-deadline challenge count at creation, and the sweep
        /// never slashes more than this many challenges per block regardless
        /// of how many deadline keys a gap matured at once — so the worst
        /// case per block equals one fully-loaded deadline. Note that
        /// consecutive parachain blocks can share a relay parent, so
        /// challenges created in different parachain blocks may share a
        /// deadline; the bound is this explicit cap, not block co-location.
        #[pallet::constant]
        type MaxChallengesPerDeadline: Get<u16>;

        /// Source of the block number every timeout, expiry and interval in
        /// this pallet is measured against. Production runtimes supply the
        /// relay chain block number
        /// (`cumulus_pallet_parachain_system::RelaychainDataProvider`) so
        /// durations stay independent of parachain block time; tests supply
        /// `frame_system::Pallet`.
        // Pinned to the parachain block-number type so the anchor and
        // parachain clocks share one concrete type (`u32` throughout).
        type BlockNumberProvider: sp_runtime::traits::BlockNumberProvider<
            BlockNumber = SystemBlockNumberFor<Self>,
        >;

        /// Milliseconds per anchor block — the tick of
        /// [`Config::BlockNumberProvider`]. Exposed via the
        /// `anchor_block_time_millis` runtime API so off-chain consumers can
        /// humanize anchor-denominated durations.
        #[pallet::constant]
        type AnchorBlockTimeMillis: Get<u64>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Storage Items
    // ─────────────────────────────────────────────────────────────────────────

    /// Provider registry.
    #[pallet::storage]
    #[pallet::getter(fn providers)]
    pub type Providers<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, ProviderInfo<T>>;

    /// Per-provider sliding replay window over signed agreement-term nonces.
    /// See [`storage_primitives::ReplayWindow`] for the bit layout
    #[pallet::storage]
    #[pallet::getter(fn provider_replay_states)]
    pub type ProviderReplayStates<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ReplayWindow, ValueQuery>;

    /// Monotonically increasing bucket ID counter.
    #[pallet::storage]
    #[pallet::getter(fn next_bucket_id)]
    pub type NextBucketId<T: Config> = StorageValue<_, BucketId, ValueQuery>;

    /// Buckets: containers for data with membership and storage agreements.
    #[pallet::storage]
    #[pallet::unbounded]
    #[pallet::getter(fn buckets)]
    pub type Buckets<T: Config> = StorageMap<_, Blake2_128Concat, BucketId, Bucket<T>>;

    /// Storage agreements: per-provider contracts for a bucket.
    #[pallet::storage]
    #[pallet::getter(fn storage_agreements)]
    pub type StorageAgreements<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        BucketId,
        Blake2_128Concat,
        T::AccountId,
        StorageAgreement<T>,
    >;

    /// Pending challenges indexed by `(deadline block, stable per-deadline
    /// index)`. The index is allocated by [`NextChallengeIndex`] and never
    /// reused for a given deadline, so a `ChallengeId { deadline, index }`
    /// stays valid even when sibling challenges sharing the same deadline are
    /// resolved (the old `Vec`-backed layout shifted indices on removal,
    /// making siblings unaddressable).
    #[pallet::storage]
    #[pallet::getter(fn challenges)]
    pub type Challenges<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        BlockNumberFor<T>,
        Twox64Concat,
        u16,
        Challenge<T>,
        OptionQuery,
    >;

    /// Next stable challenge index to allocate for a given deadline block.
    /// Monotonically increasing per deadline; never decremented when a
    /// challenge is resolved, guaranteeing index stability for siblings.
    #[pallet::storage]
    pub type NextChallengeIndex<T: Config> =
        StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, u16, ValueQuery>;

    /// Highest deadline key the `on_initialize` slash sweep has drained. Each
    /// block it sweeps up to (but excluding) the previous block's relay parent.
    /// `None` until the first block after genesis/upgrade anchors it. A cursor
    /// over anchor-denominated deadline keys, hence [`BlockNumberFor`].
    #[pallet::storage]
    pub type LastSweptChallengeBlock<T: Config> = StorageValue<_, BlockNumberFor<T>, OptionQuery>;

    /// Number of unresolved challenges currently outstanding against a
    /// provider, summed across every bucket. Incremented in `create_challenge`
    /// and decremented exactly once per resolution (defended/invalid-response
    /// in `respond_to_challenge`, or timeout in the `on_initialize` sweep). Gates
    /// `complete_deregister`: a provider cannot exit while still slashable for
    /// a pending challenge.
    #[pallet::storage]
    pub type PendingChallenges<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

    /// Number of unresolved challenges outstanding against a specific
    /// `(bucket, provider)` pair. Maintained in lockstep with
    /// [`PendingChallenges`] and gates that bucket's agreement teardown
    /// (`end_agreement`, `claim_expired_agreement`, `cleanup_bucket_internal`).
    #[pallet::storage]
    pub type PendingChallengesByBucket<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        BucketId,
        Blake2_128Concat,
        T::AccountId,
        u32,
        ValueQuery,
    >;

    /// Per-challenger aggregates so the SDK doesn't have to scan historical
    /// events to answer `get_challenge_stats`. Updated by `create_challenge`,
    /// the defended path of `respond_to_challenge`, and
    /// `slash_provider_for_failed_challenge`.
    #[pallet::storage]
    pub type ChallengerStats<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ChallengerStatRecord, ValueQuery>;

    /// Reverse index: account → bucket IDs they are a member of.
    #[pallet::storage]
    pub type MemberBuckets<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<BucketId, T::MaxBucketsPerMember>,
        ValueQuery,
    >;

    // ─────────────────────────────────────────────────────────────────────────
    // Genesis Config
    // ─────────────────────────────────────────────────────────────────────────

    /// A provider registered at genesis.
    #[derive(
        CloneNoBound,
        PartialEqNoBound,
        EqNoBound,
        DebugNoBound,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[serde(bound(serialize = "", deserialize = ""), rename_all = "camelCase")]
    pub struct GenesisProvider<T: Config> {
        /// Provider account; must be endowed with at least `stake` by the
        /// balances genesis.
        pub account: T::AccountId,
        /// Multiaddr for connecting to this provider, hex-encoded in JSON
        /// ("0x..."); must fit `T::MaxMultiaddrLength`.
        #[serde(with = "sp_core::bytes")]
        pub multiaddr: Vec<u8>,
        /// Raw public key bytes (32, 33 or 64), hex-encoded in JSON.
        #[serde(with = "sp_core::bytes")]
        pub public_key: Vec<u8>,
        /// Stake to reserve; must be at least `T::MinProviderStake`.
        pub stake: BalanceOf<T>,
        /// Provider settings, validated like `update_provider_settings`.
        pub settings: ProviderSettings<T>,
    }

    /// Genesis configuration for the storage provider pallet.
    #[pallet::genesis_config]
    #[derive(DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Buckets to create at genesis: (admin_account, min_providers).
        pub buckets: Vec<(T::AccountId, u32)>,
        /// Providers to register at genesis. Their stake is reserved from
        /// the balances-genesis endowment.
        pub providers: Vec<GenesisProvider<T>>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            for p in &self.providers {
                let multiaddr: BoundedVec<u8, T::MaxMultiaddrLength> = p
                    .multiaddr
                    .clone()
                    .try_into()
                    .expect("genesis provider multiaddr exceeds MaxMultiaddrLength");
                let public_key: BoundedVec<u8, ConstU32<64>> = p
                    .public_key
                    .clone()
                    .try_into()
                    .expect("genesis provider public key exceeds 64 bytes");
                Pallet::<T>::register_provider_internal(
                    &p.account,
                    multiaddr,
                    public_key,
                    p.stake,
                    p.settings.clone(),
                )
                .expect("genesis provider registration should not fail");
            }
            for (admin, min_providers) in &self.buckets {
                // Genesis omits the choice, so the fail-safe wrapper default applies.
                Pallet::<T>::create_bucket_internal(
                    admin,
                    *min_providers,
                    None,
                    Visibility::Private,
                )
                .expect("genesis bucket creation should not fail");
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Types
    // ─────────────────────────────────────────────────────────────────────────

    /// Provider information stored on-chain.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
    #[scale_info(skip_type_params(T))]
    pub struct ProviderInfo<T: Config> {
        /// Multiaddr for connecting to this provider.
        pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        /// Public key for signature verification.
        /// Stored as raw bytes to support multiple key types (Sr25519, Ed25519, Ecdsa).
        pub public_key: BoundedVec<u8, ConstU32<64>>,
        /// Total stake locked by this provider.
        pub stake: BalanceOf<T>,
        /// Total contracted bytes (sum of max_bytes across all agreements).
        pub committed_bytes: u64,
        /// Provider settings.
        pub settings: ProviderSettings<T>,
        /// Provider statistics.
        pub stats: ProviderStats<T>,
        /// Block at which a previously-announced deregistration becomes
        /// finalisable via `complete_deregister`. `None` means no
        /// announcement is in progress. During the announcement window the
        /// provider is still on-chain and still slashable for any pending
        /// challenge — they only get their stake back after the window.
        pub deregister_at: Option<BlockNumberFor<T>>,
    }

    /// Provider settings controlling pricing and availability.
    #[derive(
        CloneNoBound,
        PartialEqNoBound,
        EqNoBound,
        Encode,
        Decode,
        codec::DecodeWithMemTracking,
        TypeInfo,
        MaxEncodedLen,
        DebugNoBound,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[scale_info(skip_type_params(T))]
    #[serde(
        bound(serialize = "", deserialize = ""),
        rename_all = "camelCase",
        default
    )]
    pub struct ProviderSettings<T: Config> {
        /// Minimum agreement duration provider will accept.
        pub min_duration: BlockNumberFor<T>,
        /// Maximum agreement duration provider will accept.
        pub max_duration: BlockNumberFor<T>,
        /// Price per byte per block for storage.
        pub price_per_byte: BalanceOf<T>,
        /// Whether accepting new primary agreements.
        pub accepting_primary: bool,
        /// Price per successful sync confirmation, or None if not accepting replicas.
        pub replica_sync_price: Option<BalanceOf<T>>,
        /// Whether accepting extensions on existing agreements.
        pub accepting_extensions: bool,
        /// Maximum storage capacity in bytes. 0 = unlimited (backward compatible).
        /// When set, provider cannot accept agreements that would exceed this capacity.
        pub max_capacity: u64,
    }

    impl<T: Config> Default for ProviderSettings<T> {
        fn default() -> Self {
            Self {
                min_duration: Zero::zero(),
                max_duration: BlockNumberFor::<T>::max_value(),
                price_per_byte: Zero::zero(),
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0, // 0 = unlimited (backward compatible)
            }
        }
    }

    /// On-chain statistics for evaluating provider quality.
    #[derive(
        CloneNoBound,
        PartialEqNoBound,
        EqNoBound,
        Encode,
        Decode,
        TypeInfo,
        MaxEncodedLen,
        DebugNoBound,
        DefaultNoBound,
    )]
    #[scale_info(skip_type_params(T))]
    pub struct ProviderStats<T: Config> {
        /// Block when provider registered.
        pub registered_at: BlockNumberFor<T>,
        /// Total agreements ever created with this provider.
        pub agreements_total: u32,
        /// Agreements where client chose to extend.
        pub agreements_extended: u32,
        /// Agreements that expired without extension.
        pub agreements_not_extended: u32,
        /// Agreements where client burned payment.
        pub agreements_burned: u32,
        /// Total bytes ever committed across all agreements.
        pub total_bytes_committed: u64,
        /// Challenges from authorized challengers (member/agreement owner at
        /// challenge creation) that the provider responded to. Counted at
        /// resolution — a challenge resolves into exactly one of
        /// received_authorized / received_public (successfully defended) or
        /// failed (slashed).
        pub challenges_received_authorized: u32,
        /// Same, for general-public challengers.
        pub challenges_received_public: u32,
        /// Number of challenges where provider was slashed. Tier-independent
        /// and disjoint from the received counters.
        pub challenges_failed: u32,
    }

    impl<T: Config> ProviderStats<T> {
        /// Successfully defended challenges across both challenger tiers.
        pub fn challenges_defended(&self) -> u32 {
            self.challenges_received_authorized
                .saturating_add(self.challenges_received_public)
        }
    }

    /// Bucket member with role.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
    #[scale_info(skip_type_params(T))]
    pub struct Member<T: Config> {
        pub account: T::AccountId,
        pub role: Role,
    }

    /// Bucket container for data with membership and storage agreements.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
    #[scale_info(skip_type_params(T))]
    pub struct Bucket<T: Config> {
        /// Members who can interact with this bucket.
        pub members: BoundedVec<Member<T>, T::MaxMembers>,
        /// If Some, bucket is append-only from this start_seq.
        pub frozen_start_seq: Option<u64>,
        /// Minimum primary provider signatures required for checkpoint.
        pub min_providers: u32,
        /// Primary provider account IDs (limited to T::MaxPrimaryProviders).
        pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
        /// Current canonical state.
        pub snapshot: Option<BucketSnapshot<BlockNumberFor<T>>>,
        /// Historical MMR roots for replica sync validation.
        pub historical_roots: [(u32, H256); 6],
        /// Total snapshots created for this bucket.
        pub total_snapshots: u32,
        /// Read visibility (see `Visibility`). On-chain, only challenge
        /// creation reads it: `Private` restricts primary challenges to
        /// members and primary-agreement owners.
        pub visibility: Visibility,
    }

    /// Storage agreement between bucket and provider.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
    #[scale_info(skip_type_params(T))]
    pub struct StorageAgreement<T: Config> {
        /// Who owns this agreement (can top up, transfer ownership).
        pub owner: T::AccountId,
        /// Maximum bytes (quota).
        pub max_bytes: u64,
        /// Payment locked for storage.
        pub payment_locked: BalanceOf<T>,
        /// Price per byte locked at creation/last extension.
        pub price_per_byte: BalanceOf<T>,
        /// Agreement expiration.
        pub expires_at: BlockNumberFor<T>,
        /// Whether provider has blocked extensions for this agreement.
        pub extensions_blocked: bool,
        /// Provider role for this bucket.
        pub role: ProviderRole<BalanceOf<T>, BlockNumberFor<T>>,
        /// Block when agreement became active.
        pub started_at: BlockNumberFor<T>,
    }

    /// Active challenge against a provider.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
    #[scale_info(skip_type_params(T))]
    pub struct Challenge<T: Config> {
        /// Bucket containing the challenged data.
        pub bucket_id: BucketId,
        /// Provider being challenged.
        pub provider: T::AccountId,
        /// Account that issued the challenge.
        pub challenger: T::AccountId,
        /// MMR root the provider committed to.
        pub mmr_root: H256,
        /// Start sequence of the commitment.
        pub start_seq: u64,
        /// Leaf + chunk being challenged.
        pub target: ChunkLocation,
        /// Deposit locked by challenger.
        pub deposit: BalanceOf<T>,
        /// Whether the challenger was authorized (bucket member or agreement
        /// owner, via `is_authorized`) at challenge creation. Snapshotted so
        /// membership/agreement changes between creation and response cannot
        /// alter the fee split applied in `respond_to_challenge`.
        pub authorized: bool,
    }

    /// Challenge response from provider.
    #[derive(
        CloneNoBound,
        PartialEqNoBound,
        EqNoBound,
        Encode,
        Decode,
        codec::DecodeWithMemTracking,
        TypeInfo,
        DebugNoBound,
    )]
    #[scale_info(skip_type_params(T))]
    pub enum ChallengeResponse<T: Config> {
        /// Provide the chunk with proofs.
        Proof {
            chunk_data: BoundedVec<u8, T::MaxChunkSize>,
            mmr_proof: MmrProof,
            chunk_proof: MerkleProof,
        },
        /// Data was deleted - show newer commitment without this seq.
        Deleted {
            new_mmr_root: H256,
            new_start_seq: u64,
            admin: T::AccountId,
            admin_signature: sp_runtime::MultiSignature,
        },
        /// Challenged state has been superseded by canonical.
        Superseded,
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Events
    // ─────────────────────────────────────────────────────────────────────────

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        // Provider events
        ProviderRegistered {
            provider: T::AccountId,
            stake: BalanceOf<T>,
        },
        ProviderDeregistered {
            provider: T::AccountId,
            stake_returned: BalanceOf<T>,
        },
        /// Provider has announced their intention to deregister. Stake stays
        /// reserved and the provider remains on-chain (and slashable) until
        /// `complete_after`, at which point they may call `complete_deregister`.
        DeregisterAnnounced {
            provider: T::AccountId,
            complete_after: BlockNumberFor<T>,
        },
        /// Provider cancelled a previously-announced deregistration.
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
            multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        },
        ExtensionsBlocked {
            bucket_id: BucketId,
            provider: T::AccountId,
            blocked: bool,
        },

        // Bucket events
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

        // Replica events
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

        // Agreement events
        AgreementAccepted {
            bucket_id: BucketId,
            provider: T::AccountId,
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
        /// Owner redeemed provider-signed terms; bucket created and agreement
        /// opened atomically.
        StorageAgreementEstablished {
            bucket_id: BucketId,
            provider: T::AccountId,
            owner: T::AccountId,
            terms: AgreementTermsOf<T>,
            expires_at: BlockNumberFor<T>,
        },
        /// Owner redeemed provider-signed replica terms; replica agreement
        /// opened against an existing bucket.
        ReplicaAgreementEstablished {
            bucket_id: BucketId,
            provider: T::AccountId,
            owner: T::AccountId,
            terms: AgreementTermsOf<T>,
            expires_at: BlockNumberFor<T>,
        },

        // Challenge events
        ChallengeCreated {
            challenge_id: ChallengeId<BlockNumberFor<T>>,
            bucket_id: BucketId,
            provider: T::AccountId,
            challenger: T::AccountId,
            respond_by: BlockNumberFor<T>,
        },
        ChallengeDefended {
            challenge_id: ChallengeId<BlockNumberFor<T>>,
            provider: T::AccountId,
            response_time_blocks: BlockNumberFor<T>,
            challenger_cost: BalanceOf<T>,
            provider_cost: BalanceOf<T>,
        },
        ChallengeSlashed {
            challenge_id: ChallengeId<BlockNumberFor<T>>,
            provider: T::AccountId,
            slashed_amount: BalanceOf<T>,
            challenger_reward: BalanceOf<T>,
            /// Whether the provider was slashed for failing to respond
            /// (`Timeout`) or for submitting a demonstrably-false response
            /// (`InvalidProof` etc).
            reason: SlashReason,
        },
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Errors
    // ─────────────────────────────────────────────────────────────────────────

    #[pallet::error]
    pub enum Error<T> {
        // Provider errors
        ProviderAlreadyRegistered,
        ProviderNotFound,
        InsufficientStake,
        InsufficientStakeForBytes,
        ProviderHasActiveAgreements,
        ProviderNotAcceptingPrimary,
        ProviderNotAcceptingReplicas,
        ProviderNotAcceptingExtensions,
        ProviderNotSlashed,
        /// Cannot set max_capacity below current committed_bytes.
        CapacityBelowCommitted,
        /// Provider capacity exceeded on accept (committed + request > max_capacity).
        CapacityExceeded,
        /// Stake insufficient to back declared capacity.
        InsufficientStakeForCapacity,
        /// Provider settings specify `min_duration > max_duration
        MinDurationExceedsMaxDuration,
        /// Provider has already announced a deregistration; the action is
        /// rejected until they complete or cancel it.
        DeregisterAnnounced,
        /// Provider has no announced deregistration to complete or cancel.
        DeregisterNotAnnounced,
        /// `complete_deregister` called before `DeregisterAnnouncementPeriod`
        /// elapsed.
        DeregisterPeriodNotElapsed,

        // Bucket errors
        BucketNotFound,
        BucketFrozen,
        BucketNotFrozen,
        NotBucketAdmin,
        NotBucketMember,
        NotBucketWriter,
        MemberNotFound,
        CannotDemoteAdmin,
        LastAdminCannotBeRemoved,
        MaxMembersReached,
        MaxPrimaryProvidersReached,
        MinProvidersNotMet,
        InvalidMinProviders,
        NotAuthorizedForPrivateBucket,

        // Agreement errors
        AgreementNotFound,
        AgreementAlreadyExists,
        AgreementExpired,
        AgreementNotExpired,
        AgreementExtensionsBlocked,
        NotAgreementOwner,
        DurationTooShort,
        DurationTooLong,
        PaymentExceedsMax,
        CannotTerminateReplica,
        SettlementWindowPassed,

        // Replica errors
        NotReplica,
        SyncTooFrequent,
        InvalidSyncRoot,
        InsufficientSyncBalance,

        // Challenge errors
        /// The challenger is the challenged provider. A self-challenge costs
        /// nothing (the response refunds the challenger's own deposit) and
        /// would pad the defended-challenge counters behind reputation.
        SelfChallenge,
        ChallengeNotFound,
        ChallengeAlreadyExists,
        InvalidChallengeProof,
        ChallengeExpired,
        NotChallengeProvider,
        ProviderNotInSnapshot,
        LeafBeyondCanonical,
        InvalidDeletionProof,
        /// A provider with unresolved challenges (`PendingChallenges > 0`)
        /// cannot complete deregistration — they are still slashable.
        ProviderHasPendingChallenges,
        /// An agreement with an unresolved challenge against this
        /// `(bucket, provider)` cannot be torn down until the challenge
        /// resolves (defended, slashed, or timed out).
        AgreementHasPendingChallenge,
        /// `MaxChallengesPerDeadline` challenges have already been allocated
        /// for the deadline this challenge would land on. Caps the total the
        /// `on_initialize` sweep must eventually drain for a single key.
        TooManyChallengesThisBlock,

        // Checkpoint errors
        InvalidSignature,
        NoSnapshot,
        SnapshotViolatesFrozen,
        InsufficientSignatures,

        // General errors
        ArithmeticOverflow,
        InvalidMultiaddr,
        InvalidPublicKey,

        // Reverse index errors
        /// Account is a member of too many buckets.
        TooManyBucketsForMember,

        // establish_storage_agreement errors
        /// Provider signature over the SCALE-encoded terms is invalid.
        InvalidProviderSignature,
        /// Signed terms have passed their `valid_until` block.
        TermsExpired,
        /// Signed terms' `valid_until` extends beyond `now + RequestTimeout` —
        /// the provider-signed validity window cap enforced on-chain.
        TermsValidityTooLong,
        /// The terms' nonce has already been consumed inside the provider's
        /// replay window.
        NonceAlreadyUsed,
        /// The terms' nonce is older than the provider's replay window
        /// (distance from `hsn` ≥ [`storage_primitives::REPLAY_WINDOW_BITS`]).
        NonceTooOld,
        /// The terms' declared owner does not match the extrinsic origin.
        TermsOwnerMismatch,
        /// Replica terms missing from a signed quote redeemed as a replica
        /// agreement.
        MissingReplicaTerms,
        /// The terms' bucket binding does not match the redeeming extrinsic:
        /// primary terms must carry no bucket, replica terms must name the
        /// targeted bucket.
        TermsBucketMismatch,
        /// Storage agreement requested 0 byte
        InvalidMaxBytesRequest,
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Extrinsics
    // ─────────────────────────────────────────────────────────────────────────

    #[pallet::call]
    #[allow(clippy::too_many_arguments)]
    impl<T: Config> Pallet<T> {
        // ─────────────────────────────────────────────────────────────────────
        // Provider Management
        // ─────────────────────────────────────────────────────────────────────

        /// Register as a storage provider.
        ///
        /// Parameters:
        /// - `multiaddr`: Network address for clients to connect
        /// - `public_key`: Public key for signature verification (raw bytes, 32-64 bytes)
        /// - `stake`: Initial stake to lock
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register_provider())]
        pub fn register_provider(
            origin: OriginFor<T>,
            multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
            public_key: BoundedVec<u8, ConstU32<64>>,
            stake: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::register_provider_internal(
                &who,
                multiaddr,
                public_key,
                stake,
                ProviderSettings::default(),
            )
        }

        /// Add stake to an existing provider registration.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::add_stake())]
        pub fn add_stake(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider
                    .as_mut()
                    .ok_or(Error::<T>::ProviderNotFound)?;

                provider.stake = provider
                    .stake
                    .checked_add(&amount)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;

                T::Currency::reserve(&who, amount)?;

                Self::deposit_event(Event::ProviderStakeAdded {
                    provider: who.clone(),
                    amount,
                    total_stake: provider.stake,
                });

                Ok(())
            })
        }

        /// Announce intent to deregister.
        ///
        /// This is the first step of a two-step exit:
        ///
        /// 1. `deregister_provider` (this call) — marks the provider as
        ///    leaving, freezes them from accepting new agreements or
        ///    extensions, and stamps `deregister_at = now + DeregisterAnnouncementPeriod`.
        ///    Stake stays reserved; the provider remains on-chain and fully
        ///    slashable for any pending or freshly-created challenge.
        /// 2. `complete_deregister` — callable once `deregister_at` has
        ///    elapsed (by which point any challenge created up to the
        ///    announcement block has already matured, because the period
        ///    must be `> ChallengeTimeout`).
        ///
        /// The two-step flow closes the slashing race where a provider
        /// could withdraw stake between the end of their last agreement
        /// and the deadline of a challenge created against it.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::deregister_provider())]
        pub fn deregister_provider(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let anchor_block = Self::current_anchor_block();
            let complete_after =
                anchor_block.saturating_add(T::DeregisterAnnouncementPeriod::get());

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider
                    .as_mut()
                    .ok_or(Error::<T>::ProviderNotFound)?;

                ensure!(
                    provider.committed_bytes == 0,
                    Error::<T>::ProviderHasActiveAgreements
                );
                Self::ensure_provider_active(provider)?;

                // Freeze acceptance so the provider can't soak up new
                // agreements (and therefore new challenge surface) during
                // the announcement window.
                provider.settings.accepting_primary = false;
                provider.settings.accepting_extensions = false;
                provider.deregister_at = Some(complete_after);
                Ok(())
            })?;

            Self::deposit_event(Event::DeregisterAnnounced {
                provider: who,
                complete_after,
            });

            Ok(())
        }

        /// Finalise a previously-announced deregistration.
        ///
        /// Callable by the provider once `DeregisterAnnouncementPeriod` has
        /// elapsed since their `deregister_provider` call. Unreserves the
        /// remaining stake and removes the provider record.
        ///
        /// Still requires `committed_bytes == 0` — if the provider somehow
        /// re-acquired commitments mid-window (they cannot today, since
        /// announce forces `accepting_primary = false` and
        /// `update_provider_settings` is blocked during the window) the
        /// caller must wait for those agreements to end first.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::complete_deregister())]
        pub fn complete_deregister(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let provider = Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;
            let deregister_at = provider
                .deregister_at
                .ok_or(Error::<T>::DeregisterNotAnnounced)?;
            let anchor_block = Self::current_anchor_block();
            ensure!(
                anchor_block >= deregister_at,
                Error::<T>::DeregisterPeriodNotElapsed
            );
            ensure!(
                provider.committed_bytes == 0,
                Error::<T>::ProviderHasActiveAgreements
            );
            // A provider with unresolved challenges is still slashable; they
            // must not be able to exit and unreserve their stake before those
            // challenges mature. The `DeregisterAnnouncementPeriod >
            // ChallengeTimeout` invariant (see `integrity_test`) guarantees any
            // challenge created up to the announcement block resolves before
            // the wait window elapses, so this only blocks genuinely-live ones.
            ensure!(
                PendingChallenges::<T>::get(&who) == 0,
                Error::<T>::ProviderHasPendingChallenges
            );

            T::Currency::unreserve(&who, provider.stake);
            Providers::<T>::remove(&who);
            ProviderReplayStates::<T>::remove(&who);

            Self::deposit_event(Event::ProviderDeregistered {
                provider: who,
                stake_returned: provider.stake,
            });

            Ok(())
        }

        /// Cancel a previously-announced deregistration.
        ///
        /// Restores `accepting_primary` / `accepting_extensions` to `true`
        /// (mirroring what `deregister_provider` forced to `false` on
        /// announce) and clears `deregister_at`. If the provider wants
        /// different post-cancel settings they can call
        /// `update_provider_settings` afterwards.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::cancel_deregister())]
        pub fn cancel_deregister(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider
                    .as_mut()
                    .ok_or(Error::<T>::ProviderNotFound)?;
                ensure!(
                    provider.deregister_at.is_some(),
                    Error::<T>::DeregisterNotAnnounced
                );
                provider.deregister_at = None;
                provider.settings.accepting_primary = true;
                provider.settings.accepting_extensions = true;
                Ok(())
            })?;

            Self::deposit_event(Event::DeregisterCancelled { provider: who });
            Ok(())
        }

        /// Update provider settings.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::update_provider_settings())]
        pub fn update_provider_settings(
            origin: OriginFor<T>,
            settings: ProviderSettings<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                settings.min_duration <= settings.max_duration,
                Error::<T>::MinDurationExceedsMaxDuration
            );

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider
                    .as_mut()
                    .ok_or(Error::<T>::ProviderNotFound)?;

                // While deregister announcement is in flight, settings are
                // frozen — otherwise the provider could re-enable
                // `accepting_primary` and start absorbing new agreements
                // during the wait window. The caller must `cancel_deregister`
                // first.
                Self::ensure_provider_active(provider)?;

                Self::validate_settings(&settings, provider.committed_bytes, provider.stake)?;

                provider.settings = settings.clone();
                Ok(())
            })?;

            Self::deposit_event(Event::ProviderSettingsUpdated {
                provider: who,
                settings,
            });

            Ok(())
        }

        /// Update the provider's multiaddr (network endpoint).
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::update_provider_multiaddr())]
        pub fn update_provider_multiaddr(
            origin: OriginFor<T>,
            multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider
                    .as_mut()
                    .ok_or(Error::<T>::ProviderNotFound)?;

                provider.multiaddr = multiaddr.clone();
                Ok(())
            })?;

            Self::deposit_event(Event::ProviderMultiaddrUpdated {
                provider: who,
                multiaddr,
            });

            Ok(())
        }

        /// Block or unblock extensions for a specific bucket.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::block_extensions())]
        pub fn set_extensions_blocked(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            blocked: bool,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(
                Providers::<T>::contains_key(&who),
                Error::<T>::ProviderNotFound
            );

            let anchor_block = Self::current_anchor_block();

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &who,
                |maybe_agreement| -> DispatchResult {
                    let agreement = maybe_agreement
                        .as_mut()
                        .ok_or(Error::<T>::AgreementNotFound)?;
                    ensure!(
                        anchor_block < agreement.expires_at,
                        Error::<T>::AgreementExpired
                    );
                    agreement.extensions_blocked = blocked;
                    Ok(())
                },
            )?;

            Self::deposit_event(Event::ExtensionsBlocked {
                bucket_id,
                provider: who,
                blocked,
            });

            Ok(())
        }

        // ─────────────────────────────────────────────────────────────────────
        // Bucket Management
        // ─────────────────────────────────────────────────────────────────────

        // DRIFT-001 / DRIFT-002: this signed-terms flow supersedes the design
        // docs' on-chain request/accept round-trip and standalone create_bucket
        // (bucket creation is folded in here).
        /// Redeem provider-signed terms: create a bucket + primary agreement
        /// in a single call.
        ///
        /// The provider signs a SCALE-encoded [`AgreementTermsOf<T>`] off-chain;
        /// the owner submits it here. The pallet verifies the signature,
        /// rejects replays via the provider's sliding nonce window, then runs
        /// the standard provider/capacity/stake checks and opens the
        /// agreement.
        ///
        /// `visibility` sets the new bucket's read visibility (see
        /// [`Visibility`]); it is the owner's choice and not part of the
        /// provider-signed terms.
        #[pallet::call_index(17)]
        #[pallet::weight(T::WeightInfo::establish_storage_agreement())]
        pub fn establish_storage_agreement(
            origin: OriginFor<T>,
            provider: T::AccountId,
            terms: AgreementTermsOf<T>,
            sig: sp_runtime::MultiSignature,
            visibility: Visibility,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::establish_storage_agreement_internal(&who, &provider, terms, &sig, visibility)?;
            Ok(())
        }

        /// Set minimum providers required for checkpoint.
        #[pallet::call_index(11)]
        #[pallet::weight(T::WeightInfo::set_bucket_min_providers())]
        pub fn set_min_providers(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            min_providers: u32,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(&who, bucket)?;

                // Cannot exceed current primary provider count
                ensure!(
                    min_providers <= bucket.primary_providers.len() as u32,
                    Error::<T>::InvalidMinProviders
                );

                bucket.min_providers = min_providers;
                Ok(())
            })
        }

        /// Freeze bucket - make append-only (irreversible).
        #[pallet::call_index(12)]
        #[pallet::weight(T::WeightInfo::freeze_bucket())]
        pub fn freeze_bucket(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(&who, bucket)?;

                ensure!(bucket.frozen_start_seq.is_none(), Error::<T>::BucketFrozen);

                // Require snapshot with min_providers
                let snapshot = bucket.snapshot.as_ref().ok_or(Error::<T>::NoSnapshot)?;

                // Count set bits in the bitfield
                let signer_count = snapshot.count_signers();
                ensure!(
                    signer_count >= bucket.min_providers as usize,
                    Error::<T>::MinProvidersNotMet
                );

                bucket.frozen_start_seq = Some(snapshot.commitment.start_seq);

                Self::deposit_event(Event::BucketFrozen {
                    bucket_id,
                    frozen_start_seq: snapshot.commitment.start_seq,
                });

                Ok(())
            })
        }

        /// Set bucket read visibility (admin only).
        ///
        /// Flips `Public` ⇄ `Private` unconditionally in both directions — a
        /// precondition on existing replicas would hand third parties a veto
        /// over the admin. Effects are asymmetric: privatizing does not recall
        /// data already replicated, publicizing cannot be undone.
        #[pallet::call_index(16)]
        #[pallet::weight(T::WeightInfo::set_bucket_visibility())]
        pub fn set_bucket_visibility(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            visibility: Visibility,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(&who, bucket)?;

                bucket.visibility = visibility;

                Self::deposit_event(Event::BucketVisibilityChanged {
                    bucket_id,
                    visibility,
                });

                Ok(())
            })
        }

        /// Add or update a member's role.
        #[pallet::call_index(13)]
        #[pallet::weight(T::WeightInfo::set_bucket_member())]
        pub fn set_member(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            member: T::AccountId,
            role: Role,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(&who, bucket)?;

                let (target_idx, target_is_admin, admin_count) =
                    Self::locate_member(bucket, &member);
                if let Some(idx) = target_idx {
                    if target_is_admin && role != Role::Admin {
                        // Admins can only demote themselves, never another admin.
                        ensure!(member == who, Error::<T>::CannotDemoteAdmin);
                        // And even self-demotion must leave at least one admin.
                        ensure!(admin_count > 1, Error::<T>::LastAdminCannotBeRemoved);
                    }
                    bucket.members[idx].role = role;
                } else {
                    // Add new member
                    let new_member = Member {
                        account: member.clone(),
                        role,
                    };
                    bucket
                        .members
                        .try_push(new_member)
                        .map_err(|_| Error::<T>::MaxMembersReached)?;

                    // Update reverse index for new member
                    MemberBuckets::<T>::try_mutate(&member, |buckets| {
                        if !buckets.contains(&bucket_id) {
                            buckets
                                .try_push(bucket_id)
                                .map_err(|_| Error::<T>::TooManyBucketsForMember)
                        } else {
                            Ok(())
                        }
                    })?;
                }

                Self::deposit_event(Event::MemberSet {
                    bucket_id,
                    member,
                    role,
                });

                Ok(())
            })
        }

        /// Remove member from bucket.
        #[pallet::call_index(14)]
        #[pallet::weight(T::WeightInfo::remove_bucket_member())]
        pub fn remove_member(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            member: T::AccountId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(&who, bucket)?;

                let (target_idx, target_is_admin, admin_count) =
                    Self::locate_member(bucket, &member);
                let member_idx = target_idx.ok_or(Error::<T>::MemberNotFound)?;

                if target_is_admin {
                    // Admins can only remove themselves, never another admin.
                    ensure!(member == who, Error::<T>::CannotDemoteAdmin);
                    // And even self-removal must leave at least one admin.
                    ensure!(admin_count > 1, Error::<T>::LastAdminCannotBeRemoved);
                }

                bucket.members.remove(member_idx);

                // Update reverse index: remove bucket from member's list
                MemberBuckets::<T>::mutate(&member, |buckets| {
                    buckets.retain(|id| *id != bucket_id);
                });

                Self::deposit_event(Event::MemberRemoved { bucket_id, member });

                Ok(())
            })
        }

        /// Remove a slashed provider from a bucket (permissionless).
        ///
        /// Anyone can call this to clean up slashed providers.
        /// The provider must have zero stake (indicating they were slashed).
        /// Returns payment to agreement owner and removes the provider from the bucket.
        #[pallet::call_index(15)]
        #[pallet::weight(T::WeightInfo::remove_slashed())]
        pub fn remove_slashed(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Verify provider is slashed (zero stake)
            let provider_info =
                Providers::<T>::get(&provider).ok_or(Error::<T>::ProviderNotFound)?;
            ensure!(
                provider_info.stake.is_zero(),
                Error::<T>::ProviderNotSlashed
            );

            // Get and remove the agreement
            let agreement = StorageAgreements::<T>::take(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;

            // Return locked payment to owner (provider failed their duty)
            T::Currency::unreserve(&agreement.owner, agreement.payment_locked);

            // Update provider committed_bytes
            Providers::<T>::mutate(&provider, |maybe_provider| {
                if let Some(info) = maybe_provider {
                    info.committed_bytes = info.committed_bytes.saturating_sub(agreement.max_bytes);
                }
            });

            // Remove from bucket's primary providers if primary
            // TODO(no-primary-provider-left)
            if matches!(agreement.role, ProviderRole::Primary) {
                Buckets::<T>::mutate(bucket_id, |maybe_bucket| {
                    if let Some(bucket) = maybe_bucket {
                        // Capture the position before removal so the snapshot's
                        // positional signer bitfield can be re-indexed to match.
                        let pos = bucket.primary_providers.iter().position(|p| p == &provider);
                        bucket.primary_providers.retain(|p| p != &provider);
                        if let (Some(pos), Some(snapshot)) = (pos, bucket.snapshot.as_mut()) {
                            snapshot.remove_provider_bit(pos);
                        }
                    }
                });

                Self::deposit_event(Event::PrimaryProviderRemoved {
                    bucket_id,
                    provider: provider.clone(),
                    reason: RemovalReason::Slashed,
                });
            }

            Self::deposit_event(Event::SlashedProviderRemoved {
                bucket_id,
                provider,
                payment_returned_to_owner: agreement.payment_locked,
            });

            Ok(())
        }

        // ─────────────────────────────────────────────────────────────────────
        // Storage Agreements
        // ─────────────────────────────────────────────────────────────────────

        /// Redeem provider-signed terms for a replica storage agreement.
        ///
        /// The provider signs a SCALE-encoded [`AgreementTermsOf<T>`] with
        /// `replica_params: Some(_)` off-chain; the owner submits it here.
        /// The pallet verifies the signature, rejects replays via the
        /// provider's sliding nonce window, then runs the standard
        /// provider/capacity/stake checks and opens the replica agreement on
        /// an existing bucket.
        #[pallet::call_index(20)]
        #[pallet::weight(T::WeightInfo::establish_replica_agreement())]
        pub fn establish_replica_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            terms: AgreementTermsOf<T>,
            sig: sp_runtime::MultiSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::establish_replica_agreement_internal(&who, bucket_id, &provider, terms, &sig)?;
            Ok(())
        }

        /// End agreement with pay/burn decision.
        #[pallet::call_index(25)]
        #[pallet::weight(match action {
            EndAction::Pay => T::WeightInfo::end_agreement(0),
            EndAction::Burn { .. } => T::WeightInfo::end_agreement(1),
        })]
        pub fn end_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            action: EndAction,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;

            // Block teardown while a challenge is pending against this
            // `(bucket, provider)` — settling/paying out the agreement now
            // would let the provider escape a live slashable challenge.
            ensure!(
                PendingChallengesByBucket::<T>::get(bucket_id, &provider) == 0,
                Error::<T>::AgreementHasPendingChallenge
            );

            let anchor_block = Self::current_anchor_block();

            let is_early_termination = anchor_block < agreement.expires_at;

            if is_early_termination {
                // Only admin can early-terminate, and only primaries
                let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
                Self::ensure_admin(&who, &bucket)?;

                ensure!(
                    matches!(agreement.role, ProviderRole::Primary),
                    Error::<T>::CannotTerminateReplica
                );
            } else {
                // After expiry, only owner can end (within settlement window)
                ensure!(agreement.owner == who, Error::<T>::NotAgreementOwner);

                let settlement_deadline = agreement
                    .expires_at
                    .saturating_add(T::SettlementTimeout::get());
                ensure!(
                    anchor_block <= settlement_deadline,
                    Error::<T>::SettlementWindowPassed
                );
            }

            Self::finalize_agreement(
                bucket_id,
                &provider,
                &agreement,
                action,
                is_early_termination,
            )
        }

        /// Claim payment for expired agreement (provider only).
        #[pallet::call_index(26)]
        #[pallet::weight(T::WeightInfo::claim_expired_agreement())]
        pub fn claim_expired_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let agreement = StorageAgreements::<T>::get(bucket_id, &who)
                .ok_or(Error::<T>::AgreementNotFound)?;

            // Block payout while a challenge is pending against this provider
            // for this bucket — the provider must not claim and exit while
            // still slashable.
            ensure!(
                PendingChallengesByBucket::<T>::get(bucket_id, &who) == 0,
                Error::<T>::AgreementHasPendingChallenge
            );

            let anchor_block = Self::current_anchor_block();

            ensure!(
                anchor_block > agreement.expires_at,
                Error::<T>::AgreementNotExpired
            );

            let settlement_deadline = agreement
                .expires_at
                .saturating_add(T::SettlementTimeout::get());
            ensure!(
                anchor_block > settlement_deadline,
                Error::<T>::AgreementNotExpired
            );

            // Provider claims - treat as Pay
            Self::finalize_agreement(bucket_id, &who, &agreement, EndAction::Pay, false)
        }

        /// Top up quota for an existing agreement (owner only).
        ///
        /// Increases max_bytes, does not change duration.
        /// Actual payment = provider.price_per_byte * additional_bytes * remaining_duration.
        #[pallet::call_index(28)]
        #[pallet::weight(T::WeightInfo::top_up_agreement())]
        pub fn top_up_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            additional_bytes: u64,
            max_payment: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let provider_info =
                Providers::<T>::get(&provider).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &provider,
                |maybe_agreement| -> DispatchResult {
                    let agreement = maybe_agreement
                        .as_mut()
                        .ok_or(Error::<T>::AgreementNotFound)?;

                    ensure!(agreement.owner == who, Error::<T>::NotAgreementOwner);

                    let anchor_block = Self::current_anchor_block();
                    let remaining_duration = if anchor_block < agreement.expires_at {
                        agreement.expires_at.saturating_sub(anchor_block)
                    } else {
                        return Err(Error::<T>::AgreementExpired.into());
                    };

                    // Calculate payment for additional bytes over remaining duration
                    let payment = Self::calculate_payment(
                        provider_info.settings.price_per_byte,
                        additional_bytes,
                        remaining_duration,
                    )?;

                    ensure!(payment <= max_payment, Error::<T>::PaymentExceedsMax);

                    // Reserve payment
                    T::Currency::reserve(&who, payment)?;

                    // Update agreement
                    let new_max_bytes = agreement
                        .max_bytes
                        .checked_add(additional_bytes)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;

                    agreement.max_bytes = new_max_bytes;
                    agreement.payment_locked = agreement
                        .payment_locked
                        .checked_add(&payment)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;

                    // Update provider committed_bytes
                    Providers::<T>::mutate(&provider, |maybe_provider| {
                        if let Some(provider_info) = maybe_provider {
                            provider_info.committed_bytes = provider_info
                                .committed_bytes
                                .saturating_add(additional_bytes);
                        }
                    });

                    Self::deposit_event(Event::AgreementToppedUp {
                        bucket_id,
                        provider: provider.clone(),
                        amount: payment,
                        new_max_bytes,
                    });

                    Ok(())
                },
            )
        }

        /// Extend agreement duration (immediate, no provider approval needed).
        ///
        /// This:
        /// 1. Settles current period: releases payment to provider for elapsed time
        /// 2. Calculates and locks new payment for extension at current provider prices
        /// 3. Updates end date to now + additional_duration
        /// 4. Updates agreement.price_per_byte (and sync_price for replicas) to current prices
        ///
        /// Price change rules:
        /// - If provider's current price <= agreement's locked price: anyone can extend
        /// - If provider's current price > agreement's locked price: only owner can extend
        ///
        /// This enables permissionless persistence for frozen buckets while protecting
        /// owners from unwanted price increases.
        #[pallet::call_index(27)]
        #[pallet::weight(T::WeightInfo::extend_agreement())]
        pub fn extend_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            additional_duration: BlockNumberFor<T>,
            max_payment: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let provider_info =
                Providers::<T>::get(&provider).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            // Check provider is accepting extensions
            ensure!(
                provider_info.settings.accepting_extensions,
                Error::<T>::ProviderNotAcceptingExtensions
            );

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &provider,
                |maybe_agreement| -> DispatchResult {
                    let agreement = maybe_agreement
                        .as_mut()
                        .ok_or(Error::<T>::AgreementNotFound)?;

                    // Check per-agreement extension block
                    ensure!(
                        !agreement.extensions_blocked,
                        Error::<T>::AgreementExtensionsBlocked
                    );

                    // Validate duration
                    Self::validate_duration(&provider_info.settings, additional_duration)?;

                    let anchor_block = Self::current_anchor_block();

                    // Check if price increased
                    let price_increased =
                        provider_info.settings.price_per_byte > agreement.price_per_byte;

                    // If price increased, only owner can extend
                    if price_increased {
                        ensure!(agreement.owner == who, Error::<T>::NotAgreementOwner);
                    }
                    // If price same or decreased, anyone can extend (permissionless persistence)

                    // Settle current period
                    let elapsed = anchor_block.saturating_sub(agreement.started_at);
                    let _remaining = if anchor_block < agreement.expires_at {
                        agreement.expires_at.saturating_sub(anchor_block)
                    } else {
                        Zero::zero()
                    };

                    // Calculate payment for elapsed time at old rate
                    let elapsed_payment = if !elapsed.is_zero() {
                        Self::calculate_payment(
                            agreement.price_per_byte,
                            agreement.max_bytes,
                            elapsed,
                        )?
                    } else {
                        Zero::zero()
                    };

                    // Release elapsed payment to provider
                    if !elapsed_payment.is_zero() {
                        T::Currency::unreserve(&agreement.owner, elapsed_payment);
                        T::Currency::transfer(
                            &agreement.owner,
                            &provider,
                            elapsed_payment,
                            ExistenceRequirement::KeepAlive,
                        )?;
                    }

                    // Calculate new payment for extension at current rate
                    let extension_payment = Self::calculate_payment(
                        provider_info.settings.price_per_byte,
                        agreement.max_bytes,
                        additional_duration,
                    )?;

                    ensure!(
                        extension_payment <= max_payment,
                        Error::<T>::PaymentExceedsMax
                    );

                    // Lock new payment from caller (not necessarily the owner)
                    T::Currency::reserve(&who, extension_payment)?;

                    // Update agreement
                    agreement.expires_at = anchor_block.saturating_add(additional_duration);
                    agreement.started_at = anchor_block;
                    agreement.price_per_byte = provider_info.settings.price_per_byte;

                    // For replicas, also update sync_price and handle sync_balance
                    if let ProviderRole::Replica { sync_price, .. } = &mut agreement.role {
                        let new_sync_price = provider_info
                            .settings
                            .replica_sync_price
                            .ok_or(Error::<T>::ProviderNotAcceptingReplicas)?;

                        // Update sync price
                        *sync_price = new_sync_price;

                        // Note: Caller should top up sync_balance separately if needed
                    }

                    // Update payment_locked (subtract old remaining, add new)
                    agreement.payment_locked = agreement
                        .payment_locked
                        .saturating_sub(elapsed_payment)
                        .saturating_add(extension_payment);

                    // Update provider stats
                    Providers::<T>::mutate(&provider, |maybe_provider| {
                        if let Some(provider_info) = maybe_provider {
                            provider_info.stats.agreements_extended =
                                provider_info.stats.agreements_extended.saturating_add(1);
                        }
                    });

                    Self::deposit_event(Event::AgreementExtended {
                        bucket_id,
                        provider: provider.clone(),
                        new_expires_at: agreement.expires_at,
                        payment: extension_payment,
                    });

                    Ok(())
                },
            )
        }

        // ─────────────────────────────────────────────────────────────────────
        // Checkpoints
        // ─────────────────────────────────────────────────────────────────────

        /// Submit a new checkpoint with provider signatures.
        #[pallet::call_index(30)]
        #[pallet::weight(T::WeightInfo::checkpoint())]
        pub fn checkpoint(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            commitment: Commitment,
            signatures: BoundedVec<
                (T::AccountId, sp_runtime::MultiSignature),
                T::MaxPrimaryProviders,
            >,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                // Must be writer or admin
                Self::ensure_writer_or_admin(&who, bucket)?;

                // Check frozen constraint
                if let Some(frozen_start) = bucket.frozen_start_seq {
                    ensure!(
                        commitment.start_seq >= frozen_start,
                        Error::<T>::SnapshotViolatesFrozen
                    );
                }

                // Verify signatures and build signer bitfield
                let payload = CommitmentPayload::new(bucket_id, commitment);
                let encoded_payload = payload.encode();

                // Create bitfield using Vec<u8>
                let num_providers = bucket.primary_providers.len();
                let num_bytes = num_providers.div_ceil(8);
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

                    // Verify the signature using the provider's registered public key
                    Self::verify_signature(signature, &encoded_payload, signer)?;

                    // Set bit at position idx using manual bit manipulation
                    let byte_idx = idx / 8;
                    let bit_idx = idx % 8;
                    primary_signers[byte_idx] |= 1 << bit_idx;
                    signing_count += 1;
                    signing_providers.push(signer.clone());
                }

                // Check min_providers
                ensure!(
                    signing_count >= bucket.min_providers as usize,
                    Error::<T>::InsufficientSignatures
                );

                let anchor_block = Self::current_anchor_block();

                // Update historical roots
                Self::update_historical_roots(bucket, anchor_block, commitment.mmr_root);

                bucket.snapshot = Some(BucketSnapshot {
                    commitment,
                    checkpoint_block: anchor_block,
                    primary_signers,
                });

                bucket.total_snapshots = bucket.total_snapshots.saturating_add(1);

                Self::deposit_event(Event::BucketCheckpointed {
                    bucket_id,
                    commitment,
                    providers: signing_providers,
                });

                Ok(())
            })
        }

        /// Add additional provider signatures to existing checkpoint.
        ///
        /// Allows late-signing providers to add their signatures to the current
        /// snapshot. Useful when a provider signs off-chain commitments later.
        #[pallet::call_index(31)]
        #[pallet::weight(T::WeightInfo::extend_checkpoint())]
        pub fn extend_checkpoint(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            additional_signatures: BoundedVec<
                (T::AccountId, sp_runtime::MultiSignature),
                T::MaxPrimaryProviders,
            >,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                // Must be writer or admin
                Self::ensure_writer_or_admin(&who, bucket)?;

                // Must have existing snapshot
                let snapshot = bucket.snapshot.as_mut().ok_or(Error::<T>::NoSnapshot)?;

                // Verify and add signatures. The late signer signs the same
                // payload the original signers signed, reconstructed from the
                // snapshot's commitment.
                let payload = CommitmentPayload::new(bucket_id, snapshot.commitment);
                let encoded_payload = payload.encode();

                let mut primary_signers = snapshot.primary_signers.clone();
                let mut added_providers = Vec::new();

                for (signer, signature) in additional_signatures.iter() {
                    // Find signer in primary_providers
                    let idx = bucket
                        .primary_providers
                        .iter()
                        .position(|p| p == signer)
                        .ok_or(Error::<T>::ProviderNotInSnapshot)?;

                    // Check if already signed using bit manipulation
                    let byte_idx = idx / 8;
                    let bit_idx = idx % 8;
                    if let Some(byte) = primary_signers.get(byte_idx) {
                        if (byte & (1 << bit_idx)) != 0 {
                            continue; // Skip already-signed providers
                        }
                    }

                    // Verify signature
                    Self::verify_signature(signature, &encoded_payload, signer)?;

                    // Set bit using manual bit manipulation
                    if byte_idx < primary_signers.len() {
                        primary_signers[byte_idx] |= 1 << bit_idx;
                    }
                    added_providers.push(signer.clone());
                }

                // Update snapshot
                snapshot.primary_signers = primary_signers;

                Self::deposit_event(Event::BucketCheckpointed {
                    bucket_id,
                    commitment: snapshot.commitment,
                    providers: added_providers,
                });

                Ok(())
            })
        }
        // ─────────────────────────────────────────────────────────────────────
        // Challenges
        // ─────────────────────────────────────────────────────────────────────

        /// Challenge on-chain checkpoint (no signatures needed).
        ///
        /// Provider must be in current snapshot's provider list.
        ///
        /// NOTE: May race with new checkpoints in hot buckets. If the provider is
        /// no longer in the snapshot when the transaction executes, this fails.
        /// For hot buckets, prefer challenge_offchain with the signature you have.
        #[pallet::call_index(40)]
        #[pallet::weight(T::WeightInfo::challenge_checkpoint())]
        pub fn challenge_checkpoint(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            target: ChunkLocation,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;
            let snapshot = bucket.snapshot.as_ref().ok_or(Error::<T>::NoSnapshot)?;

            // Verify provider is in snapshot
            let provider_idx = bucket
                .primary_providers
                .iter()
                .position(|p| p == &provider)
                .ok_or(Error::<T>::ProviderNotInSnapshot)?;

            // Check if provider bit is set in the bitfield
            let provider_signed = snapshot.has_provider_signed(provider_idx);
            ensure!(provider_signed, Error::<T>::ProviderNotInSnapshot);

            // Verify provider has an ACTIVE agreement for this bucket. As with
            // `challenge_offchain`/`challenge_replica`, challengeability must
            // track genuine obligation: a challenge can only open while the
            // agreement is live (not into the settlement window), so an expired
            // checkpoint can no longer be challenged.
            let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;
            ensure!(
                Self::current_anchor_block() < agreement.expires_at,
                Error::<T>::AgreementExpired
            );

            Self::create_challenge(
                who,
                bucket_id,
                &bucket,
                provider,
                &agreement.role,
                snapshot.commitment,
                target,
            )
        }

        /// Challenge off-chain commitment (requires provider signature).
        ///
        /// Works regardless of current snapshot state - the signature proves
        /// the provider committed to this data.
        ///
        /// Preferred for hot buckets where snapshots change frequently.
        #[pallet::call_index(42)]
        #[pallet::weight(T::WeightInfo::challenge_off_chain())]
        pub fn challenge_offchain(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            // `commitment` carries the `(mmr_root, start_seq, leaf_count)` the
            // provider signed. The challenger passes it through so the payload
            // reconstruction matches the signed `CommitmentPayload` exactly.
            commitment: Commitment,
            // `target` is the leaf+chunk being challenged within `commitment`.
            target: ChunkLocation,
            provider_signature: sp_runtime::MultiSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Verify the bucket exists (loaded in full: the private-bucket
            // gate and the challenger tier read its visibility and members).
            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            // Verify provider has an ACTIVE agreement for this bucket. An
            // expired-but-unswept agreement leaves a stale row in
            // `StorageAgreements`; challengeability must track genuine
            // obligation, so a challenge can only open while the agreement is
            // live (not into the settlement window).
            let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;
            ensure!(
                Self::current_anchor_block() < agreement.expires_at,
                Error::<T>::AgreementExpired
            );

            // Build the commitment payload that the provider signed.
            //
            // TODO(#316): the liveness check above is keyed `(bucket_id,
            // provider)`, which a re-join reuses, so it cannot tell a
            // commitment signed under the previous agreement from one signed
            // under the current one. A provider that left, legitimately
            // deleted the data and re-joined is still slashable on the old
            // commitment. The removed nonce recency window (#337) used to cap
            // that at 24h; the real bound is the agreement's lifetime, which
            // arrives with `agreement_id` in #316.
            let payload = CommitmentPayload::new(bucket_id, commitment);
            let encoded_payload = payload.encode();

            // Verify the provider's signature on this commitment
            Self::verify_signature(&provider_signature, &encoded_payload, &provider)?;

            // Create the challenge
            Self::create_challenge(
                who,
                bucket_id,
                &bucket,
                provider,
                &agreement.role,
                commitment,
                target,
            )
        }

        /// Challenge a replica based on their on-chain sync confirmation.
        ///
        /// Uses the replica's last_synced_root stored in their agreement.
        /// No signature needed - the chain already has their commitment.
        #[pallet::call_index(43)]
        #[pallet::weight(T::WeightInfo::challenge_replica())]
        pub fn challenge_replica(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            target: ChunkLocation,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get the agreement and verify it's a replica
            let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;

            // Challengeability tracks genuine obligation: only while the
            // agreement is live (not into the settlement window).
            ensure!(
                Self::current_anchor_block() < agreement.expires_at,
                Error::<T>::AgreementExpired
            );

            let commitment = match &agreement.role {
                ProviderRole::Replica { last_sync, .. } => {
                    last_sync
                        .as_ref()
                        .ok_or(Error::<T>::InvalidSyncRoot)?
                        .commitment
                }
                ProviderRole::Primary => return Err(Error::<T>::NotReplica.into()),
            };

            // Needed for the challenger-tier snapshot (replicas are never
            // gated by visibility, but the tier still applies).
            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            Self::create_challenge(
                who,
                bucket_id,
                &bucket,
                provider,
                &agreement.role,
                commitment,
                target,
            )
        }

        /// Respond to a challenge.
        #[pallet::call_index(41)]
        #[pallet::weight(match response {
            ChallengeResponse::Proof { .. } => T::WeightInfo::respond_to_challenge_proof(),
            ChallengeResponse::Deleted { .. } => T::WeightInfo::respond_to_challenge_deleted(),
            ChallengeResponse::Superseded => T::WeightInfo::respond_to_challenge_superseded(),
        })]
        pub fn respond_to_challenge(
            origin: OriginFor<T>,
            challenge_id: ChallengeId<BlockNumberFor<T>>,
            response: ChallengeResponse<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Consume the challenge up front. With the stable-index DoubleMap
            // a single `take` removes exactly this challenge and leaves its
            // siblings (sharing the same deadline) untouched and addressable.
            // Any `?`-bail below (wrong provider, expired, missing bucket)
            // reverts the extrinsic, rolling the `take` back so the challenge
            // remains pending; only the adjudicated `response_outcome` (which
            // never short-circuits with `?`) commits the removal.
            let challenge = Challenges::<T>::take(challenge_id.deadline, challenge_id.index)
                .ok_or(Error::<T>::ChallengeNotFound)?;

            // The `take` consumes this challenge, so resolve the pending
            // counters now — this covers BOTH the defended path and the
            // invalid-response slash path below. Any `?`-bail after this point
            // reverts the whole extrinsic (including this decrement and the
            // `take`), so the challenge and its counters stay in lockstep.
            Self::decrement_pending(challenge.bucket_id, &challenge.provider);

            ensure!(challenge.provider == who, Error::<T>::NotChallengeProvider);

            let anchor_block = Self::current_anchor_block();
            ensure!(
                anchor_block <= challenge_id.deadline,
                Error::<T>::ChallengeExpired
            );

            // Verify response
            let bucket =
                Buckets::<T>::get(challenge.bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            // Adjudicate the response. Returns:
            //   `Ok(())`       — response defends the challenge
            //   `Err(reason)`  — response is a demonstrable lie; slash the
            //                    provider immediately (do NOT let them stall
            //                    until the deadline timeout)
            //
            // Parameter-shape errors (stale nonce, non-admin signer, missing
            // bucket snapshot for `Deleted`) still bubble up as `DispatchError`
            // — they represent caller mistakes, not adversarial responses.
            let response_outcome: Result<(), SlashReason> = match &response {
                ChallengeResponse::Proof {
                    chunk_data,
                    mmr_proof,
                    chunk_proof,
                } => {
                    let chunk_hash = storage_primitives::blake2_256(chunk_data);
                    let chunk_ok = storage_primitives::verify_merkle_proof(
                        chunk_hash,
                        challenge.target.chunk_index,
                        chunk_proof,
                        &mmr_proof.leaf.data_root,
                    );
                    let mmr_ok =
                        storage_primitives::verify_mmr_proof(mmr_proof, &challenge.mmr_root);
                    if chunk_ok && mmr_ok {
                        Ok(())
                    } else {
                        Err(SlashReason::InvalidProof)
                    }
                }
                ChallengeResponse::Deleted {
                    new_mmr_root,
                    new_start_seq,
                    admin,
                    admin_signature,
                } => {
                    Self::ensure_admin(admin, &bucket)?;

                    let challenged_seq = challenge
                        .start_seq
                        .saturating_add(challenge.target.leaf_index);
                    if challenged_seq >= *new_start_seq {
                        // Provider claims data was purged before the
                        // challenged leaf, but the new start_seq doesn't
                        // actually cover it.
                        Err(SlashReason::InvalidDeletionClaim)
                    } else {
                        let deletion_payload = CommitmentPayload::new(
                            challenge.bucket_id,
                            Commitment {
                                mmr_root: *new_mmr_root,
                                start_seq: *new_start_seq,
                                leaf_count: 0, // not needed for deletion proof
                            },
                        );
                        let encoded = deletion_payload.encode();
                        if Self::verify_signature(admin_signature, &encoded, admin).is_ok() {
                            Ok(())
                        } else {
                            Err(SlashReason::InvalidDeletionClaim)
                        }
                    }
                }
                ChallengeResponse::Superseded => {
                    // A `Superseded` defense only holds when the challenged
                    // commitment was genuinely replaced by a newer canonical
                    // snapshot. Without a snapshot to lean on the claim is
                    // unsupported, so we slash.
                    match bucket.snapshot.as_ref() {
                        None => Err(SlashReason::InvalidSupersededClaim),
                        Some(snapshot) => {
                            let challenged_seq = challenge
                                .start_seq
                                .saturating_add(challenge.target.leaf_index);
                            // (a) The challenged root must NOT be the current
                            // canonical root — if it still is, the data is live
                            // and the provider must answer with a `Proof`.
                            // (b)+(c) The challenged seq must still sit inside
                            // the canonical range; front-rolled/deleted data
                            // has to go through the admin-signed `Deleted` path.
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
            };

            // The challenge was already removed by the `take` above; the owned
            // `challenge` value feeds either the defended-path cost-split or
            // the slash helper. The adjudication has concluded, so the
            // removal now becomes the committed state transition.

            if let Err(reason) = response_outcome {
                // Invalid response → slash now. The extrinsic itself returns
                // `Ok(())` because the slash *is* the valid state transition;
                // the provider is the one paying the price, recorded via the
                // `ChallengeSlashed { reason, .. }` event.
                Self::slash_provider_for_failed_challenge(&challenge, challenge_id, reason);
                return Ok(());
            }

            // Calculate response time (blocks since challenge was created)
            let challenge_created_at = challenge_id
                .deadline
                .saturating_sub(T::ChallengeTimeout::get());
            let response_time = anchor_block.saturating_sub(challenge_created_at);

            // A valid response never touches the provider's stake — only the
            // response cost (proxied by the deposit) is at issue, and the
            // challenger's deposit reimburses it. How much of that cost the
            // provider is made to bear depends on the tier snapshotted at
            // creation:
            // - General public pays in full — the provider bears nothing
            //   (anti-DDoS: if strangers got the split, a crowd could each pay
            //   little while collectively draining the provider).
            // - Authorized challengers (members/agreement owners) get a split
            //   by response time, floored at 50% — leverage, not cheap
            //   recovery:
            //   Block 1: 90/10, 2-5: 80/20, 6-24: 70/30, 25-95: 60/40,
            //   96+: 50/50 (challenger/provider).
            let challenger_percent = if !challenge.authorized {
                100u32
            } else if response_time <= BlockNumberFor::<T>::from(1u32) {
                90u32
            } else if response_time <= BlockNumberFor::<T>::from(5u32) {
                80u32
            } else if response_time <= BlockNumberFor::<T>::from(24u32) {
                70u32
            } else if response_time <= BlockNumberFor::<T>::from(95u32) {
                60u32
            } else {
                50u32
            };

            let challenger_cost = challenge.deposit * challenger_percent.into() / 100u32.into();
            // The provider's share is borne by simply not being reimbursed for
            // it — no funds move from the provider, and its stake stays intact.
            let provider_cost = challenge.deposit.saturating_sub(challenger_cost);

            // Challenger forfeits `challenger_cost` to the provider as
            // compensation for the work of responding: move it from the
            // challenger's reserved balance into the provider's free balance.
            let not_moved = T::Currency::repatriate_reserved(
                &challenge.challenger,
                &challenge.provider,
                challenger_cost,
                BalanceStatus::Free,
            )
            .unwrap_or(challenger_cost);
            // Refund challenger the rest of their deposit. Anything that could
            // not be moved (should not happen) is released back to them too, so
            // no funds stay stuck in the challenger's reserved balance.
            let refund = challenge
                .deposit
                .saturating_sub(challenger_cost)
                .saturating_add(not_moved);
            T::Currency::unreserve(&challenge.challenger, refund);

            // Count the responded-to challenge per tier (resolution-time
            // stats; creation leaves no trace).
            Providers::<T>::mutate(&who, |maybe_provider| {
                if let Some(provider) = maybe_provider {
                    if challenge.authorized {
                        provider.stats.challenges_received_authorized = provider
                            .stats
                            .challenges_received_authorized
                            .saturating_add(1);
                    } else {
                        provider.stats.challenges_received_public =
                            provider.stats.challenges_received_public.saturating_add(1);
                    }
                }
            });

            // Challenger lost — they pay `challenger_cost` from their deposit
            // and the provider keeps their stake. Bump the failed counter so
            // the SDK can report a realistic success rate.
            ChallengerStats::<T>::mutate(&challenge.challenger, |stats| {
                stats.failed_challenges = stats.failed_challenges.saturating_add(1);
            });

            Self::deposit_event(Event::ChallengeDefended {
                challenge_id,
                provider: who,
                response_time_blocks: response_time,
                challenger_cost,
                provider_cost,
            });

            Ok(())
        }

        // ─────────────────────────────────────────────────────────────────────
        // Replica Sync
        // ─────────────────────────────────────────────────────────────────────

        /// Replica confirms sync to MMR roots.
        #[pallet::call_index(50)]
        #[pallet::weight(T::WeightInfo::confirm_replica_sync())]
        pub fn confirm_replica_sync(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            roots: [Option<H256>; 7],
            _signature: sp_runtime::MultiSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &who,
                |maybe_agreement| -> DispatchResult {
                    let agreement = maybe_agreement
                        .as_mut()
                        .ok_or(Error::<T>::AgreementNotFound)?;

                    let (sync_balance, sync_price, min_sync_interval, last_sync) =
                        match &mut agreement.role {
                            ProviderRole::Replica {
                                sync_balance,
                                sync_price,
                                min_sync_interval,
                                last_sync,
                            } => (sync_balance, sync_price, min_sync_interval, last_sync),
                            ProviderRole::Primary => return Err(Error::<T>::NotReplica.into()),
                        };

                    let anchor_block = Self::current_anchor_block();

                    // Check sync interval
                    if let Some(record) = last_sync {
                        let min_next_block = record.block.saturating_add(*min_sync_interval);
                        ensure!(anchor_block >= min_next_block, Error::<T>::SyncTooFrequent);
                    }

                    // Find matching root position
                    let (position_matched, matched_root) =
                        Self::find_matching_root(&bucket, &roots)?;

                    // Check it's a new root
                    if let Some(record) = last_sync {
                        ensure!(
                            matched_root != record.commitment.mmr_root,
                            Error::<T>::InvalidSyncRoot
                        );
                    }

                    // Pay for sync
                    ensure!(
                        *sync_balance >= *sync_price,
                        Error::<T>::InsufficientSyncBalance
                    );
                    *sync_balance = sync_balance.saturating_sub(*sync_price);

                    // Capture sequence metadata for the matched root so a
                    // future `challenge_replica` can target a specific leaf.
                    // For the current snapshot (position_matched == 0) we
                    // know start_seq + leaf_count exactly. Historical roots
                    // don't carry sequence metadata in `historical_roots`, so
                    // they default to 0 here — challenges targeting a leaf
                    // beyond seq 0 in that case still work because
                    // `challenge_replica` only uses `start_seq` as an offset
                    // additive identity.
                    let (start_seq, leaf_count) = if position_matched == 0 {
                        bucket
                            .snapshot
                            .as_ref()
                            .map(|s| (s.commitment.start_seq, s.commitment.leaf_count))
                            .unwrap_or((0, 0))
                    } else {
                        (0u64, 0u64)
                    };

                    // Update last sync
                    *last_sync = Some(ReplicaSyncRecord {
                        commitment: Commitment {
                            mmr_root: matched_root,
                            start_seq,
                            leaf_count,
                        },
                        block: anchor_block,
                    });

                    // Transfer sync payment to provider
                    T::Currency::unreserve(&agreement.owner, *sync_price);
                    T::Currency::transfer(
                        &agreement.owner,
                        &who,
                        *sync_price,
                        ExistenceRequirement::KeepAlive,
                    )?;

                    Self::deposit_event(Event::ReplicaSynced {
                        bucket_id,
                        provider: who.clone(),
                        mmr_root: matched_root,
                        position_matched,
                        sync_payment: *sync_price,
                    });

                    Ok(())
                },
            )
        }

        /// Top up a replica's sync balance.
        #[pallet::call_index(51)]
        #[pallet::weight(T::WeightInfo::top_up_replica_sync_balance())]
        pub fn top_up_replica_sync_balance(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            amount: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            T::Currency::reserve(&who, amount)?;

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &provider,
                |maybe_agreement| -> DispatchResult {
                    let agreement = maybe_agreement
                        .as_mut()
                        .ok_or(Error::<T>::AgreementNotFound)?;

                    let sync_balance = match &mut agreement.role {
                        ProviderRole::Replica { sync_balance, .. } => sync_balance,
                        ProviderRole::Primary => return Err(Error::<T>::NotReplica.into()),
                    };

                    *sync_balance = sync_balance
                        .checked_add(&amount)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;

                    Self::deposit_event(Event::ReplicaSyncBalanceToppedUp {
                        bucket_id,
                        provider: provider.clone(),
                        amount,
                        new_total: *sync_balance,
                    });

                    Ok(())
                },
            )
        }
    }
}
