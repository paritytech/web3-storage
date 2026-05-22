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
pub mod pallet {
    use crate::weights::WeightInfo;
    use alloc::vec;
    use alloc::vec::Vec;
    use frame_support::{
        pallet_prelude::*,
        traits::{Currency, ExistenceRequirement, ReservableCurrency},
        CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
    };
    use frame_system::pallet_prelude::*;
    use sp_core::H256;
    use sp_runtime::traits::{Bounded, CheckedAdd, Saturating, Zero};
    use storage_primitives::{
        BucketId, BucketSnapshot, ChallengeId, CommitmentPayload, EndAction, MerkleProof, MmrProof,
        ProviderRole, RemovalReason, ReplicaRequestParams, Role, HISTORICAL_ROOT_PRIMES,
    };

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Process expired challenges at the end of each block.
        fn on_finalize(n: BlockNumberFor<T>) {
            // Check if there are any challenges expiring at this block
            if let Some(expired_challenges) = Challenges::<T>::take(n) {
                for (index, challenge) in expired_challenges.iter().enumerate() {
                    // Slash the provider for failing to respond
                    let challenge_id = ChallengeId {
                        deadline: n,
                        index: index as u16,
                    };
                    Self::slash_provider_for_failed_challenge(challenge, challenge_id);
                }
            }
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

        /// Timeout for challenge response (e.g., ~48 hours in blocks).
        #[pallet::constant]
        type ChallengeTimeout: Get<BlockNumberFor<Self>>;

        /// Settlement window after agreement expiry for owner to call end_agreement.
        #[pallet::constant]
        type SettlementTimeout: Get<BlockNumberFor<Self>>;

        /// Maximum duration for agreement requests before expiry.
        #[pallet::constant]
        type RequestTimeout: Get<BlockNumberFor<Self>>;

        /// Default interval between provider-initiated checkpoints (e.g., 100 blocks).
        #[pallet::constant]
        type DefaultCheckpointInterval: Get<BlockNumberFor<Self>>;

        /// Default grace period for checkpoint leader (e.g., 20 blocks).
        #[pallet::constant]
        type DefaultCheckpointGrace: Get<BlockNumberFor<Self>>;

        /// Reward paid to provider for submitting a checkpoint.
        #[pallet::constant]
        type CheckpointReward: Get<BalanceOf<Self>>;

        /// Penalty for missing a checkpoint window (slashed from provider stake).
        #[pallet::constant]
        type CheckpointMissPenalty: Get<BalanceOf<Self>>;

        /// Maximum number of buckets a single account can be a member of.
        #[pallet::constant]
        type MaxBucketsPerMember: Get<u32>;

        /// Minimum number of blocks between announcing a deregistration and
        /// being allowed to complete it. Must be `>= ChallengeTimeout` so any
        /// challenge against this provider that was created up to the
        /// announcement block matures while the provider is still slashable.
        #[pallet::constant]
        type DeregisterAnnouncementPeriod: Get<BlockNumberFor<Self>>;

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

    /// Pending agreement requests (bucket → provider → request).
    #[pallet::storage]
    #[pallet::getter(fn agreement_requests)]
    pub type AgreementRequests<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        BucketId,
        Blake2_128Concat,
        T::AccountId,
        AgreementRequest<T>,
    >;

    /// Pending challenges indexed by deadline block.
    #[pallet::storage]
    #[pallet::unbounded]
    #[pallet::getter(fn challenges)]
    pub type Challenges<T: Config> =
        StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, Vec<Challenge<T>>>;

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

    /// Genesis configuration for the storage provider pallet.
    #[pallet::genesis_config]
    #[derive(DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Buckets to create at genesis: (admin_account, min_providers).
        pub buckets: Vec<(T::AccountId, u32)>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            for (admin, min_providers) in &self.buckets {
                Pallet::<T>::create_bucket_internal(admin, *min_providers)
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
    )]
    #[scale_info(skip_type_params(T))]
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
        /// Number of challenges received.
        pub challenges_received: u32,
        /// Number of challenges where provider was slashed.
        pub challenges_failed: u32,
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

    /// Pending agreement request.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
    #[scale_info(skip_type_params(T))]
    pub struct AgreementRequest<T: Config> {
        /// Who requested the agreement.
        pub requester: T::AccountId,
        /// Maximum bytes requested.
        pub max_bytes: u64,
        /// Payment locked by requester.
        pub payment_locked: BalanceOf<T>,
        /// Requested duration.
        pub duration: BlockNumberFor<T>,
        /// Block at which request expires.
        pub expires_at: BlockNumberFor<T>,
        /// Replica-specific parameters, None for primary agreements.
        pub replica_params: Option<ReplicaRequestParams<BalanceOf<T>, BlockNumberFor<T>>>,
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
        /// Leaf index within the MMR.
        pub leaf_index: u64,
        /// Chunk index within the leaf's data.
        pub chunk_index: u64,
        /// Deposit locked by challenger.
        pub deposit: BalanceOf<T>,
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
            mmr_root: H256,
            start_seq: u64,
            leaf_count: u64,
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
        },

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
        /// Provider settings specify `min_duration > max_duration`, which
        /// would silently brick the provider in `find_matching_provider`.
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
        MaxMembersReached,
        MaxPrimaryProvidersReached,
        MinProvidersNotMet,
        InvalidMinProviders,

        // Agreement errors
        AgreementNotFound,
        AgreementRequestNotFound,
        AgreementAlreadyExists,
        AgreementRequestAlreadyExists,
        AgreementExpired,
        AgreementNotExpired,
        AgreementExtensionsBlocked,
        NotAgreementOwner,
        DurationTooShort,
        DurationTooLong,
        PaymentExceedsMax,
        RequestExpired,
        CannotTerminateReplica,
        SettlementWindowPassed,

        // Replica errors
        NotReplica,
        SyncTooFrequent,
        InvalidSyncRoot,
        InsufficientSyncBalance,

        // Challenge errors
        ChallengeNotFound,
        ChallengeAlreadyExists,
        InvalidChallengeProof,
        ChallengeExpired,
        NotChallengeProvider,
        ProviderNotInSnapshot,
        LeafBeyondCanonical,
        InvalidDeletionProof,

        // Checkpoint errors
        InvalidSignature,
        NoSnapshot,
        SnapshotViolatesFrozen,
        InsufficientSignatures,

        // General errors
        ArithmeticOverflow,
        InvalidMultiaddr,
        InvalidPublicKey,

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

        // Reverse index errors
        /// Account is a member of too many buckets.
        TooManyBucketsForMember,

        // Auto-matching errors
        /// No provider found matching the storage requirements.
        NoMatchingProvider,
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Extrinsics
    // ─────────────────────────────────────────────────────────────────────────

    #[pallet::call]
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

            ensure!(
                !Providers::<T>::contains_key(&who),
                Error::<T>::ProviderAlreadyRegistered
            );
            ensure!(
                stake >= T::MinProviderStake::get(),
                Error::<T>::InsufficientStake
            );

            // Validate public key length (32 bytes for Sr25519/Ed25519, 33 for Ecdsa compressed)
            let key_len = public_key.len();
            ensure!(
                key_len == 32 || key_len == 33 || key_len == 64,
                Error::<T>::InvalidPublicKey
            );

            // Reserve stake
            T::Currency::reserve(&who, stake)?;

            let current_block = frame_system::Pallet::<T>::block_number();

            let provider_info = ProviderInfo {
                multiaddr,
                public_key,
                stake,
                committed_bytes: 0,
                settings: ProviderSettings::default(),
                stats: ProviderStats {
                    registered_at: current_block,
                    ..Default::default()
                },
                deregister_at: None,
            };

            Providers::<T>::insert(&who, provider_info);

            Self::deposit_event(Event::ProviderRegistered {
                provider: who,
                stake,
            });

            Ok(())
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
        ///    must be `>= ChallengeTimeout`).
        ///
        /// The two-step flow closes the slashing race where a provider
        /// could withdraw stake between the end of their last agreement
        /// and the deadline of a challenge created against it.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::deregister_provider())]
        pub fn deregister_provider(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let current_block = frame_system::Pallet::<T>::block_number();
            let complete_after =
                current_block.saturating_add(T::DeregisterAnnouncementPeriod::get());

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
        /// elapsed since their `deregister_provider` call. Drains any
        /// pending `CheckpointRewards` into the provider's free balance,
        /// unreserves the remaining stake, and removes the provider record.
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
            let current_block = frame_system::Pallet::<T>::block_number();
            ensure!(
                current_block >= deregister_at,
                Error::<T>::DeregisterPeriodNotElapsed
            );
            ensure!(
                provider.committed_bytes == 0,
                Error::<T>::ProviderHasActiveAgreements
            );

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

            T::Currency::unreserve(&who, provider.stake);
            Providers::<T>::remove(&who);

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

                // Validate max_capacity >= committed_bytes (unless 0 = unlimited)
                if settings.max_capacity > 0 {
                    ensure!(
                        settings.max_capacity >= provider.committed_bytes,
                        Error::<T>::CapacityBelowCommitted
                    );

                    // Validate stake backs declared capacity
                    use sp_runtime::traits::SaturatedConversion;
                    let capacity_as_balance: BalanceOf<T> = settings.max_capacity.saturated_into();
                    let required_stake = T::MinStakePerByte::get()
                        .checked_mul(&capacity_as_balance)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;
                    ensure!(
                        provider.stake >= required_stake,
                        Error::<T>::InsufficientStakeForCapacity
                    );
                }

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

                provider.multiaddr = multiaddr;
                Ok(())
            })?;

            Self::deposit_event(Event::ProviderMultiaddrUpdated { provider: who });

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

            let current_block = frame_system::Pallet::<T>::block_number();

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &who,
                |maybe_agreement| -> DispatchResult {
                    let agreement = maybe_agreement
                        .as_mut()
                        .ok_or(Error::<T>::AgreementNotFound)?;
                    ensure!(
                        current_block < agreement.expires_at,
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

        /// Create a new bucket.
        #[pallet::call_index(10)]
        #[pallet::weight(T::WeightInfo::create_bucket())]
        pub fn create_bucket(origin: OriginFor<T>, min_providers: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket_id = NextBucketId::<T>::get();
            NextBucketId::<T>::put(bucket_id.saturating_add(1));

            let admin_member = Member {
                account: who.clone(),
                role: Role::Admin,
            };

            let mut members = BoundedVec::new();
            members
                .try_push(admin_member)
                .map_err(|_| Error::<T>::MaxMembersReached)?;

            let bucket = Bucket {
                members,
                frozen_start_seq: None,
                min_providers,
                primary_providers: BoundedVec::new(),
                snapshot: None,
                historical_roots: [(0, H256::zero()); 6],
                total_snapshots: 0,
            };

            Buckets::<T>::insert(bucket_id, bucket);

            // Update reverse index for creator
            MemberBuckets::<T>::try_mutate(&who, |buckets| {
                buckets
                    .try_push(bucket_id)
                    .map_err(|_| Error::<T>::TooManyBucketsForMember)
            })?;

            Self::deposit_event(Event::BucketCreated {
                bucket_id,
                admin: who,
            });

            Ok(())
        }

        /// Create a new bucket with storage requirements and auto-match to a provider.
        ///
        /// This is the preferred way to create a bucket with storage. The system
        /// automatically finds a matching provider based on your requirements and
        /// creates both the bucket and agreement in one atomic operation.
        ///
        /// Providers who set `accepting_primary: true` have pre-consented to accepting
        /// agreements within their stated parameters (capacity, price, duration).
        #[pallet::call_index(16)]
        #[pallet::weight(T::WeightInfo::create_bucket_with_storage())]
        pub fn create_bucket_with_storage(
            origin: OriginFor<T>,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
            max_price_per_byte: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Find a matching provider
            let (provider, provider_info) =
                Self::find_matching_provider(max_bytes, duration, max_price_per_byte)?;

            // Calculate payment using provider's actual price
            let payment = Self::calculate_payment(
                provider_info.settings.price_per_byte,
                max_bytes,
                duration,
            )?;

            // Reserve funds from caller
            T::Currency::reserve(&who, payment)?;

            // Create the bucket
            let bucket_id = NextBucketId::<T>::get();
            NextBucketId::<T>::put(bucket_id.saturating_add(1));

            let admin_member = Member {
                account: who.clone(),
                role: Role::Admin,
            };

            let mut members = BoundedVec::new();
            members
                .try_push(admin_member)
                .map_err(|_| Error::<T>::MaxMembersReached)?;

            let mut primary_providers = BoundedVec::new();
            primary_providers
                .try_push(provider.clone())
                .map_err(|_| Error::<T>::MaxPrimaryProvidersReached)?;

            let bucket = Bucket {
                members,
                frozen_start_seq: None,
                min_providers: 1,
                primary_providers,
                snapshot: None,
                historical_roots: [(0, H256::zero()); 6],
                total_snapshots: 0,
            };

            Buckets::<T>::insert(bucket_id, bucket);

            // Update reverse index for creator
            MemberBuckets::<T>::try_mutate(&who, |buckets| {
                buckets
                    .try_push(bucket_id)
                    .map_err(|_| Error::<T>::TooManyBucketsForMember)
            })?;

            // Create the agreement
            let current_block = frame_system::Pallet::<T>::block_number();
            let expires_at = current_block.saturating_add(duration);

            let agreement = StorageAgreement {
                owner: who.clone(),
                max_bytes,
                payment_locked: payment,
                price_per_byte: provider_info.settings.price_per_byte,
                expires_at,
                extensions_blocked: false,
                role: ProviderRole::Primary,
                started_at: current_block,
            };

            // Update provider's committed_bytes
            Providers::<T>::mutate(&provider, |maybe_provider| {
                if let Some(provider_info) = maybe_provider {
                    provider_info.committed_bytes =
                        provider_info.committed_bytes.saturating_add(max_bytes);
                    provider_info.stats.agreements_total =
                        provider_info.stats.agreements_total.saturating_add(1);
                }
            });

            StorageAgreements::<T>::insert(bucket_id, &provider, agreement);

            // Emit events
            Self::deposit_event(Event::BucketCreated {
                bucket_id,
                admin: who.clone(),
            });

            Self::deposit_event(Event::AgreementAccepted {
                bucket_id,
                provider: provider.clone(),
                expires_at,
            });

            Self::deposit_event(Event::ProviderAddedToBucket {
                bucket_id,
                provider,
            });

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

                bucket.frozen_start_seq = Some(snapshot.start_seq);

                Self::deposit_event(Event::BucketFrozen {
                    bucket_id,
                    frozen_start_seq: snapshot.start_seq,
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

                // Find if member already exists
                if let Some(existing) = bucket.members.iter_mut().find(|m| m.account == member) {
                    // Cannot demote other admins (only yourself)
                    // TODO(no-admin-left)
                    // this allow the sole admin self-demote, potentially leaving
                    // the bucket with no admins.
                    if existing.role == Role::Admin && member != who {
                        return Err(Error::<T>::CannotDemoteAdmin.into());
                    }
                    existing.role = role;
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

                let member_idx = bucket
                    .members
                    .iter()
                    .position(|m| m.account == member)
                    .ok_or(Error::<T>::MemberNotFound)?;

                // Cannot remove other admins
                // TODO(no-admin-left)
                if bucket.members[member_idx].role == Role::Admin && member != who {
                    return Err(Error::<T>::CannotDemoteAdmin.into());
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
            // TODO(no-admin-left)
            if matches!(agreement.role, ProviderRole::Primary) {
                Buckets::<T>::mutate(bucket_id, |maybe_bucket| {
                    if let Some(bucket) = maybe_bucket {
                        bucket.primary_providers.retain(|p| p != &provider);
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

        /// Request a replica storage agreement.
        #[pallet::call_index(20)]
        #[pallet::weight(T::WeightInfo::request_agreement())]
        pub fn request_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
            max_payment: BalanceOf<T>,
            replica_params: ReplicaRequestParams<BalanceOf<T>, BlockNumberFor<T>>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(
                Buckets::<T>::contains_key(bucket_id),
                Error::<T>::BucketNotFound
            );

            let provider_info =
                Providers::<T>::get(&provider).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            ensure!(
                provider_info.settings.replica_sync_price.is_some(),
                Error::<T>::ProviderNotAcceptingReplicas
            );

            Self::validate_duration(&provider_info.settings, duration)?;

            // Calculate payment
            let payment = Self::calculate_payment(
                provider_info.settings.price_per_byte,
                max_bytes,
                duration,
            )?;
            ensure!(payment <= max_payment, Error::<T>::PaymentExceedsMax);

            // Total to lock = storage payment + sync balance
            let total_lock = payment
                .checked_add(&replica_params.sync_balance)
                .ok_or(Error::<T>::ArithmeticOverflow)?;

            // Reserve funds
            T::Currency::reserve(&who, total_lock)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            let expires_at = current_block.saturating_add(T::RequestTimeout::get());

            let request = AgreementRequest {
                requester: who.clone(),
                max_bytes,
                payment_locked: payment,
                duration,
                expires_at,
                replica_params: Some(replica_params),
            };

            ensure!(
                !AgreementRequests::<T>::contains_key(bucket_id, &provider),
                Error::<T>::AgreementRequestAlreadyExists
            );

            AgreementRequests::<T>::insert(bucket_id, &provider, request);

            Self::deposit_event(Event::AgreementRequested {
                bucket_id,
                provider,
                requester: who,
                max_bytes,
                payment_locked: payment,
                duration,
            });

            Ok(())
        }

        /// Request a primary storage agreement (admin only).
        #[pallet::call_index(21)]
        #[pallet::weight(T::WeightInfo::request_primary_agreement())]
        pub fn request_primary_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
            max_payment: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            Self::ensure_admin(&who, &bucket)?;

            // Check primary provider limit
            ensure!(
                bucket.primary_providers.len() < T::MaxPrimaryProviders::get() as usize,
                Error::<T>::MaxPrimaryProvidersReached
            );

            let provider_info =
                Providers::<T>::get(&provider).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            ensure!(
                provider_info.settings.accepting_primary,
                Error::<T>::ProviderNotAcceptingPrimary
            );

            Self::validate_duration(&provider_info.settings, duration)?;

            let payment = Self::calculate_payment(
                provider_info.settings.price_per_byte,
                max_bytes,
                duration,
            )?;
            ensure!(payment <= max_payment, Error::<T>::PaymentExceedsMax);

            T::Currency::reserve(&who, payment)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            let expires_at = current_block.saturating_add(T::RequestTimeout::get());

            let request = AgreementRequest {
                requester: who.clone(),
                max_bytes,
                payment_locked: payment,
                duration,
                expires_at,
                replica_params: None, // Primary agreement
            };

            ensure!(
                !AgreementRequests::<T>::contains_key(bucket_id, &provider),
                Error::<T>::AgreementRequestAlreadyExists
            );

            AgreementRequests::<T>::insert(bucket_id, &provider, request);

            Self::deposit_event(Event::AgreementRequested {
                bucket_id,
                provider,
                requester: who,
                max_bytes,
                payment_locked: payment,
                duration,
            });

            Ok(())
        }

        /// Accept a pending agreement request.
        #[pallet::call_index(22)]
        #[pallet::weight(T::WeightInfo::accept_agreement())]
        pub fn accept_agreement(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let provider_info = Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            let request = AgreementRequests::<T>::take(bucket_id, &who)
                .ok_or(Error::<T>::AgreementRequestNotFound)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            ensure!(
                current_block <= request.expires_at,
                Error::<T>::RequestExpired
            );

            let expires_at = current_block.saturating_add(request.duration);

            // Create the role based on whether replica params exist
            let role = if let Some(replica_params) = request.replica_params {
                let provider_info =
                    Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;
                let sync_price = provider_info
                    .settings
                    .replica_sync_price
                    .ok_or(Error::<T>::ProviderNotAcceptingReplicas)?;

                ProviderRole::Replica {
                    sync_balance: replica_params.sync_balance,
                    sync_price,
                    min_sync_interval: replica_params.min_sync_interval,
                    last_sync: None,
                }
            } else {
                // Primary: add to bucket's primary_providers
                Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                    let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;
                    bucket
                        .primary_providers
                        .try_push(who.clone())
                        .map_err(|_| Error::<T>::MaxPrimaryProvidersReached)?;
                    Ok(())
                })?;

                ProviderRole::Primary
            };

            let provider_info = Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;

            // Enforce stake-to-bytes ratio
            // New commitment = existing + requested
            let new_committed_bytes = provider_info
                .committed_bytes
                .checked_add(request.max_bytes)
                .ok_or(Error::<T>::ArithmeticOverflow)?;

            // Check capacity constraint (if max_capacity > 0)
            if provider_info.settings.max_capacity > 0 {
                ensure!(
                    new_committed_bytes <= provider_info.settings.max_capacity,
                    Error::<T>::CapacityExceeded
                );
            }

            // Required stake = committed_bytes * min_stake_per_byte
            // Using saturated multiplication to avoid overflow
            use sp_runtime::traits::SaturatedConversion;
            let bytes_as_balance: BalanceOf<T> = new_committed_bytes.saturated_into();
            let required_stake = T::MinStakePerByte::get()
                .checked_mul(&bytes_as_balance)
                .ok_or(Error::<T>::ArithmeticOverflow)?;

            ensure!(
                provider_info.stake >= required_stake,
                Error::<T>::InsufficientStakeForBytes
            );

            let agreement = StorageAgreement {
                owner: request.requester,
                max_bytes: request.max_bytes,
                payment_locked: request.payment_locked,
                price_per_byte: provider_info.settings.price_per_byte,
                expires_at,
                extensions_blocked: false,
                role,
                started_at: current_block,
            };

            // Update provider stats
            Providers::<T>::mutate(&who, |maybe_provider| {
                if let Some(provider) = maybe_provider {
                    provider.committed_bytes =
                        provider.committed_bytes.saturating_add(request.max_bytes);
                    provider.stats.agreements_total =
                        provider.stats.agreements_total.saturating_add(1);
                    provider.stats.total_bytes_committed = provider
                        .stats
                        .total_bytes_committed
                        .saturating_add(request.max_bytes);
                }
            });

            StorageAgreements::<T>::insert(bucket_id, &who, agreement);

            Self::deposit_event(Event::AgreementAccepted {
                bucket_id,
                provider: who.clone(),
                expires_at,
            });

            Self::deposit_event(Event::ProviderAddedToBucket {
                bucket_id,
                provider: who,
            });

            Ok(())
        }

        /// Reject a pending agreement request.
        #[pallet::call_index(23)]
        #[pallet::weight(T::WeightInfo::reject_agreement())]
        pub fn reject_agreement(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let request = AgreementRequests::<T>::take(bucket_id, &who)
                .ok_or(Error::<T>::AgreementRequestNotFound)?;

            // Calculate total locked (storage payment + sync balance for replicas)
            let total_locked = if let Some(ref replica_params) = request.replica_params {
                request
                    .payment_locked
                    .checked_add(&replica_params.sync_balance)
                    .ok_or(Error::<T>::ArithmeticOverflow)?
            } else {
                request.payment_locked
            };

            // Return funds to requester
            T::Currency::unreserve(&request.requester, total_locked);

            Self::deposit_event(Event::AgreementRejected {
                bucket_id,
                provider: who,
                payment_returned: total_locked,
            });

            Ok(())
        }

        /// Withdraw a pending agreement request.
        #[pallet::call_index(24)]
        #[pallet::weight(T::WeightInfo::withdraw_agreement_request())]
        pub fn withdraw_agreement_request(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let request = AgreementRequests::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementRequestNotFound)?;

            ensure!(request.requester == who, Error::<T>::NotAgreementOwner);

            AgreementRequests::<T>::remove(bucket_id, &provider);

            // Calculate total locked
            let total_locked = if let Some(ref replica_params) = request.replica_params {
                request
                    .payment_locked
                    .checked_add(&replica_params.sync_balance)
                    .ok_or(Error::<T>::ArithmeticOverflow)?
            } else {
                request.payment_locked
            };

            T::Currency::unreserve(&who, total_locked);

            Self::deposit_event(Event::AgreementRequestWithdrawn {
                bucket_id,
                provider,
                payment_returned: total_locked,
            });

            Ok(())
        }

        /// End agreement with pay/burn decision.
        #[pallet::call_index(25)]
        #[pallet::weight(T::WeightInfo::end_agreement())]
        pub fn end_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            action: EndAction,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;

            let current_block = frame_system::Pallet::<T>::block_number();

            let is_early_termination = current_block < agreement.expires_at;

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
                    current_block <= settlement_deadline,
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

            let current_block = frame_system::Pallet::<T>::block_number();

            ensure!(
                current_block > agreement.expires_at,
                Error::<T>::AgreementNotExpired
            );

            let settlement_deadline = agreement
                .expires_at
                .saturating_add(T::SettlementTimeout::get());
            ensure!(
                current_block > settlement_deadline,
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

                    let current_block = frame_system::Pallet::<T>::block_number();
                    let remaining_duration = if current_block < agreement.expires_at {
                        agreement.expires_at.saturating_sub(current_block)
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

                    let current_block = frame_system::Pallet::<T>::block_number();

                    // Check if price increased
                    let price_increased =
                        provider_info.settings.price_per_byte > agreement.price_per_byte;

                    // If price increased, only owner can extend
                    if price_increased {
                        ensure!(agreement.owner == who, Error::<T>::NotAgreementOwner);
                    }
                    // If price same or decreased, anyone can extend (permissionless persistence)

                    // Settle current period
                    let elapsed = current_block.saturating_sub(agreement.started_at);
                    let _remaining = if current_block < agreement.expires_at {
                        agreement.expires_at.saturating_sub(current_block)
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
                    agreement.expires_at = current_block.saturating_add(additional_duration);
                    agreement.started_at = current_block;
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
            mmr_root: H256,
            start_seq: u64,
            leaf_count: u64,
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
                        start_seq >= frozen_start,
                        Error::<T>::SnapshotViolatesFrozen
                    );
                }

                // Verify signatures and build signer bitfield
                let payload = CommitmentPayload::new(bucket_id, mmr_root, start_seq, leaf_count);
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

                let current_block = frame_system::Pallet::<T>::block_number();

                // Update historical roots
                Self::update_historical_roots(bucket, current_block, mmr_root);

                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root,
                    start_seq,
                    leaf_count,
                    checkpoint_block: current_block,
                    primary_signers,
                });

                bucket.total_snapshots = bucket.total_snapshots.saturating_add(1);

                Self::deposit_event(Event::BucketCheckpointed {
                    bucket_id,
                    mmr_root,
                    start_seq,
                    leaf_count,
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

                // Verify and add signatures
                let payload = CommitmentPayload::new(
                    bucket_id,
                    snapshot.mmr_root,
                    snapshot.start_seq,
                    snapshot.leaf_count,
                );
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
                    mmr_root: snapshot.mmr_root,
                    start_seq: snapshot.start_seq,
                    leaf_count: snapshot.leaf_count,
                    providers: added_providers,
                });

                Ok(())
            })
        }

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
            mmr_root: H256,
            start_seq: u64,
            leaf_count: u64,
            window: u64,
            signatures: BoundedVec<
                (T::AccountId, sp_runtime::MultiSignature),
                T::MaxPrimaryProviders,
            >,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get checkpoint config
            let config = Self::get_checkpoint_config(bucket_id);
            ensure!(config.enabled, Error::<T>::ProviderCheckpointsDisabled);

            // Get current block and calculate current window
            let current_block = frame_system::Pallet::<T>::block_number();
            let current_window = Self::calculate_window(current_block, config.interval);

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
                let within_grace = Self::is_within_grace_period(current_block, window, &config);
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
                Self::update_historical_roots(bucket, current_block, mmr_root);

                // Update bucket snapshot
                bucket.snapshot = Some(BucketSnapshot {
                    mmr_root,
                    start_seq,
                    leaf_count,
                    checkpoint_block: current_block,
                    primary_signers,
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
            let current_block = frame_system::Pallet::<T>::block_number();
            let current_window = Self::calculate_window(current_block, config.interval);

            // Can only report past windows
            ensure!(window < current_window, Error::<T>::InvalidCheckpointWindow);

            // Check that window wasn't submitted
            if let Some(last_window) = LastCheckpointWindow::<T>::get(bucket_id) {
                ensure!(window > last_window, Error::<T>::CheckpointAlreadySubmitted);
            }

            // Ensure we're past the grace period of the reported window
            let window_end = Self::window_start_block(window.saturating_add(1), config.interval);
            ensure!(current_block > window_end, Error::<T>::WithinGracePeriod);

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
            leaf_index: u64,
            chunk_index: u64,
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

            Self::create_challenge(
                who,
                bucket_id,
                provider,
                snapshot.mmr_root,
                snapshot.start_seq,
                leaf_index,
                chunk_index,
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
        #[allow(clippy::too_many_arguments)]
        pub fn challenge_offchain(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
            mmr_root: H256,
            start_seq: u64,
            leaf_index: u64,
            chunk_index: u64,
            provider_signature: sp_runtime::MultiSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Verify the bucket exists
            ensure!(
                Buckets::<T>::contains_key(bucket_id),
                Error::<T>::BucketNotFound
            );

            // Verify provider has an agreement for this bucket
            ensure!(
                StorageAgreements::<T>::contains_key(bucket_id, &provider),
                Error::<T>::AgreementNotFound
            );

            // Build the commitment payload that the provider signed
            // Note: We use leaf_count = 0 here as a placeholder since we don't have it
            // The actual verification will be based on the mmr_proof submitted in the response
            let payload = CommitmentPayload::new(bucket_id, mmr_root, start_seq, 0);
            let encoded_payload = payload.encode();

            // Verify the provider's signature on this commitment
            Self::verify_signature(&provider_signature, &encoded_payload, &provider)?;

            // Create the challenge
            Self::create_challenge(
                who,
                bucket_id,
                provider,
                mmr_root,
                start_seq,
                leaf_index,
                chunk_index,
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
            leaf_index: u64,
            chunk_index: u64,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Get the agreement and verify it's a replica
            let agreement = StorageAgreements::<T>::get(bucket_id, &provider)
                .ok_or(Error::<T>::AgreementNotFound)?;

            let (mmr_root, start_seq) = match &agreement.role {
                ProviderRole::Replica { last_sync, .. } => {
                    let (root, _block) = last_sync.as_ref().ok_or(Error::<T>::InvalidSyncRoot)?;
                    // We need to get the start_seq from the bucket's snapshot at that root
                    // For simplicity, we'll use 0 here - in production this should be tracked
                    (*root, 0u64)
                }
                ProviderRole::Primary => return Err(Error::<T>::NotReplica.into()),
            };

            Self::create_challenge(
                who,
                bucket_id,
                provider,
                mmr_root,
                start_seq,
                leaf_index,
                chunk_index,
            )
        }

        /// Respond to a challenge.
        #[pallet::call_index(41)]
        #[pallet::weight(T::WeightInfo::respond_to_challenge())]
        pub fn respond_to_challenge(
            origin: OriginFor<T>,
            challenge_id: ChallengeId<BlockNumberFor<T>>,
            response: ChallengeResponse<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let mut challenges =
                Challenges::<T>::get(challenge_id.deadline).ok_or(Error::<T>::ChallengeNotFound)?;

            let challenge = challenges
                .get(challenge_id.index as usize)
                .ok_or(Error::<T>::ChallengeNotFound)?;

            ensure!(challenge.provider == who, Error::<T>::NotChallengeProvider);

            let current_block = frame_system::Pallet::<T>::block_number();
            ensure!(
                current_block <= challenge_id.deadline,
                Error::<T>::ChallengeExpired
            );

            // Verify response
            let bucket =
                Buckets::<T>::get(challenge.bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            match &response {
                ChallengeResponse::Proof {
                    chunk_data,
                    mmr_proof,
                    chunk_proof,
                } => {
                    // Verify chunk hash
                    let chunk_hash = storage_primitives::blake2_256(chunk_data);

                    // Verify chunk is in data_root
                    ensure!(
                        storage_primitives::verify_merkle_proof(
                            chunk_hash,
                            challenge.chunk_index,
                            chunk_proof,
                            &mmr_proof.leaf.data_root,
                        ),
                        Error::<T>::InvalidChallengeProof
                    );

                    // Verify MMR proof: leaf is in the MMR with the challenged root
                    ensure!(
                        storage_primitives::verify_mmr_proof(mmr_proof, &challenge.mmr_root),
                        Error::<T>::InvalidChallengeProof
                    );
                }
                ChallengeResponse::Deleted {
                    new_mmr_root,
                    new_start_seq,
                    admin,
                    admin_signature,
                } => {
                    // Verify admin is bucket admin
                    Self::ensure_admin(admin, &bucket)?;

                    // Verify challenged seq is before new start
                    let challenged_seq = challenge.start_seq.saturating_add(challenge.leaf_index);
                    ensure!(
                        challenged_seq < *new_start_seq,
                        Error::<T>::InvalidDeletionProof
                    );

                    // Verify admin signature on the deletion commitment
                    let deletion_payload = CommitmentPayload::new(
                        challenge.bucket_id,
                        *new_mmr_root,
                        *new_start_seq,
                        0, // leaf_count not needed for deletion proof
                    );
                    let encoded = deletion_payload.encode();
                    Self::verify_signature(admin_signature, &encoded, admin)?;
                }
                ChallengeResponse::Superseded => {
                    let snapshot = bucket.snapshot.as_ref().ok_or(Error::<T>::NoSnapshot)?;
                    let challenged_seq = challenge.start_seq.saturating_add(challenge.leaf_index);
                    let canonical_end = snapshot.start_seq.saturating_add(snapshot.leaf_count);

                    ensure!(
                        challenged_seq < canonical_end,
                        Error::<T>::LeafBeyondCanonical
                    );
                }
            }

            // Challenge defended - calculate costs based on response time
            let challenge = challenges.remove(challenge_id.index as usize);

            // Update or remove the challenges list
            if challenges.is_empty() {
                Challenges::<T>::remove(challenge_id.deadline);
            } else {
                Challenges::<T>::insert(challenge_id.deadline, challenges);
            }

            // Calculate response time (blocks since challenge was created)
            let challenge_created_at = challenge_id
                .deadline
                .saturating_sub(T::ChallengeTimeout::get());
            let response_time = current_block.saturating_sub(challenge_created_at);

            // Calculate cost split based on response time
            // Per design:
            // Block 1: Challenger 90%, Provider 10%
            // Blocks 2-5: Challenger 80%, Provider 20%
            // Blocks 6-24: Challenger 70%, Provider 30%
            // Blocks 25-95: Challenger 60%, Provider 40%
            // Blocks 96+: Challenger 50%, Provider 50%

            let challenger_percent = if response_time <= BlockNumberFor::<T>::from(1u32) {
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

            let provider_percent = 100u32.saturating_sub(challenger_percent);

            // Calculate actual costs
            let challenger_cost = challenge.deposit * challenger_percent.into() / 100u32.into();
            let provider_cost = challenge.deposit * provider_percent.into() / 100u32.into();

            // Refund challenger (deposit minus their cost)
            let refund = challenge.deposit.saturating_sub(challenger_cost);
            T::Currency::unreserve(&challenge.challenger, refund);

            // Slash provider_cost from provider's stake
            // Note: In on_finalize we can't easily handle errors, but here we can
            let (_, remaining) = T::Currency::slash_reserved(&who, provider_cost);
            let actually_slashed = provider_cost.saturating_sub(remaining);

            // Update provider stake in storage
            Providers::<T>::mutate(&who, |maybe_provider| {
                if let Some(provider) = maybe_provider {
                    provider.stake = provider.stake.saturating_sub(actually_slashed);
                }
            });

            Self::deposit_event(Event::ChallengeDefended {
                challenge_id,
                provider: who,
                response_time_blocks: response_time,
                challenger_cost,
                provider_cost: actually_slashed,
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

                    let current_block = frame_system::Pallet::<T>::block_number();

                    // Check sync interval
                    if let Some((_, last_block)) = last_sync {
                        let min_next_block = last_block.saturating_add(*min_sync_interval);
                        ensure!(current_block >= min_next_block, Error::<T>::SyncTooFrequent);
                    }

                    // Find matching root position
                    let (position_matched, matched_root) =
                        Self::find_matching_root(&bucket, &roots)?;

                    // Check it's a new root
                    if let Some((old_root, _)) = last_sync {
                        ensure!(matched_root != *old_root, Error::<T>::InvalidSyncRoot);
                    }

                    // Pay for sync
                    ensure!(
                        *sync_balance >= *sync_price,
                        Error::<T>::InsufficientSyncBalance
                    );
                    *sync_balance = sync_balance.saturating_sub(*sync_price);

                    // Update last sync
                    *last_sync = Some((matched_root, current_block));

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

    // ─────────────────────────────────────────────────────────────────────────
    // Helper Functions
    // ─────────────────────────────────────────────────────────────────────────

    impl<T: Config> Pallet<T> {
        /// Verify a MultiSignature against an encoded message using stored public key.
        ///
        /// This:
        /// 1. Retrieves the provider's registered public key from storage
        /// 2. Reconstructs the appropriate public key type from raw bytes
        /// 3. Verifies the signature matches the message and public key
        ///
        /// Returns Error::InvalidSignature if verification fails.
        fn verify_signature(
            signature: &sp_runtime::MultiSignature,
            message: &[u8],
            signer: &T::AccountId,
        ) -> DispatchResult {
            use sp_runtime::traits::Verify;

            // Get the provider's registered public key
            let provider = Providers::<T>::get(signer).ok_or(Error::<T>::ProviderNotFound)?;
            let public_key_bytes = provider.public_key.as_slice();

            // Convert public key to AccountId32 based on signature type
            let account_id = match signature {
                sp_runtime::MultiSignature::Sr25519(_) | sp_runtime::MultiSignature::Ed25519(_) => {
                    // Sr25519 and Ed25519 public keys are 32 bytes, directly used as AccountId32
                    if public_key_bytes.len() != 32 {
                        return Err(Error::<T>::InvalidPublicKey.into());
                    }
                    let mut key_bytes = [0u8; 32];
                    key_bytes.copy_from_slice(public_key_bytes);
                    sp_runtime::AccountId32::new(key_bytes)
                }
                sp_runtime::MultiSignature::Ecdsa(_) | sp_runtime::MultiSignature::Eth(_) => {
                    // Ecdsa/Eth public keys are 33 bytes (compressed), AccountId32 is blake2_256 hash
                    if public_key_bytes.len() != 33 {
                        return Err(Error::<T>::InvalidPublicKey.into());
                    }
                    let hash = sp_io::hashing::blake2_256(public_key_bytes);
                    sp_runtime::AccountId32::new(hash)
                }
            };

            // Verify signature against the account ID
            let is_valid = signature.verify(message, &account_id);

            ensure!(is_valid, Error::<T>::InvalidSignature);

            Ok(())
        }

        fn ensure_admin(who: &T::AccountId, bucket: &Bucket<T>) -> DispatchResult {
            ensure!(
                bucket
                    .members
                    .iter()
                    .any(|m| &m.account == who && m.role == Role::Admin),
                Error::<T>::NotBucketAdmin
            );
            Ok(())
        }

        fn ensure_writer_or_admin(who: &T::AccountId, bucket: &Bucket<T>) -> DispatchResult {
            ensure!(
                bucket.members.iter().any(|m| &m.account == who
                    && (m.role == Role::Admin || m.role == Role::Writer)),
                Error::<T>::NotBucketWriter
            );
            Ok(())
        }

        /// Reject any path that would create a new commitment for a
        /// provider who has announced deregistration. `deregister_provider`
        /// also flips `accepting_primary`/`accepting_extensions` to `false`,
        fn ensure_provider_active(provider: &ProviderInfo<T>) -> DispatchResult {
            ensure!(
                provider.deregister_at.is_none(),
                Error::<T>::DeregisterAnnounced
            );
            Ok(())
        }

        /// Add or update a member's role on a bucket (callable from other pallets).
        ///
        /// The `caller` must be an Admin of the bucket.
        pub fn set_member_internal(
            caller: &T::AccountId,
            bucket_id: BucketId,
            member: T::AccountId,
            role: Role,
        ) -> DispatchResult {
            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(caller, bucket)?;

                if let Some(existing) = bucket.members.iter_mut().find(|m| m.account == member) {
                    // TODO(no-admin-left)
                    if existing.role == Role::Admin && member != *caller {
                        return Err(Error::<T>::CannotDemoteAdmin.into());
                    }
                    existing.role = role;
                } else {
                    let new_member = Member {
                        account: member.clone(),
                        role,
                    };
                    bucket
                        .members
                        .try_push(new_member)
                        .map_err(|_| Error::<T>::MaxMembersReached)?;

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

        /// Remove a member from a bucket (callable from other pallets).
        ///
        /// The `caller` must be an Admin of the bucket.
        pub fn remove_member_internal(
            caller: &T::AccountId,
            bucket_id: BucketId,
            member: T::AccountId,
        ) -> DispatchResult {
            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(caller, bucket)?;

                let member_idx = bucket
                    .members
                    .iter()
                    .position(|m| m.account == member)
                    .ok_or(Error::<T>::MemberNotFound)?;

                if bucket.members[member_idx].role == Role::Admin && member != *caller {
                    return Err(Error::<T>::CannotDemoteAdmin.into());
                }

                bucket.members.remove(member_idx);

                MemberBuckets::<T>::mutate(&member, |buckets| {
                    buckets.retain(|id| *id != bucket_id);
                });

                Self::deposit_event(Event::MemberRemoved { bucket_id, member });

                Ok(())
            })
        }

        fn validate_duration(
            settings: &ProviderSettings<T>,
            duration: BlockNumberFor<T>,
        ) -> DispatchResult {
            ensure!(
                duration >= settings.min_duration,
                Error::<T>::DurationTooShort
            );
            ensure!(
                duration <= settings.max_duration,
                Error::<T>::DurationTooLong
            );
            Ok(())
        }

        fn calculate_payment(
            price_per_byte: BalanceOf<T>,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
        ) -> Result<BalanceOf<T>, DispatchError> {
            // payment = price_per_byte * max_bytes * duration
            // Use saturated_from for type conversions
            use sp_runtime::traits::SaturatedConversion;
            let bytes_balance: BalanceOf<T> = max_bytes.saturated_into();
            let duration_u128: u128 = duration.saturated_into();
            let duration_balance: BalanceOf<T> = duration_u128.saturated_into();

            price_per_byte
                .checked_mul(&bytes_balance)
                .and_then(|p| p.checked_mul(&duration_balance))
                .ok_or(Error::<T>::ArithmeticOverflow.into())
        }

        /// Find a provider matching the storage requirements.
        ///
        /// Returns the best matching provider that:
        /// - Is accepting primary agreements
        /// - Has sufficient available capacity
        /// - Has price at or below max_price_per_byte
        /// - Accepts the requested duration
        /// - Has sufficient stake to back the additional bytes
        fn find_matching_provider(
            bytes_needed: u64,
            duration: BlockNumberFor<T>,
            max_price_per_byte: BalanceOf<T>,
        ) -> Result<(T::AccountId, ProviderInfo<T>), DispatchError> {
            use sp_runtime::traits::SaturatedConversion;

            let mut best_match: Option<(T::AccountId, ProviderInfo<T>, BalanceOf<T>)> = None;

            for (account, info) in Providers::<T>::iter() {
                // Skip providers in the middle of deregistering. The flag
                // check below also catches this (announce forces it false),
                // but check explicitly so we don't depend on flag-mutation
                // ordering for the security guarantee.
                if info.deregister_at.is_some() {
                    continue;
                }

                // Must be accepting primary agreements
                if !info.settings.accepting_primary {
                    continue;
                }

                // Check duration constraints
                if duration < info.settings.min_duration || duration > info.settings.max_duration {
                    continue;
                }

                // Check price constraint
                if info.settings.price_per_byte > max_price_per_byte {
                    continue;
                }

                // Check capacity constraint
                let max_capacity = info.settings.max_capacity;
                if max_capacity > 0 {
                    let available = max_capacity.saturating_sub(info.committed_bytes);
                    if available < bytes_needed {
                        continue;
                    }
                }

                // Check stake constraint (can they back the additional bytes?)
                let new_committed = info.committed_bytes.saturating_add(bytes_needed);
                let bytes_as_balance: BalanceOf<T> = new_committed.saturated_into();
                if let Some(required_stake) =
                    T::MinStakePerByte::get().checked_mul(&bytes_as_balance)
                {
                    if info.stake < required_stake {
                        continue;
                    }
                } else {
                    continue;
                }

                // This provider matches! Track best by lowest price
                let price = info.settings.price_per_byte;
                match &best_match {
                    None => {
                        best_match = Some((account, info, price));
                    }
                    Some((_, _, best_price)) if price < *best_price => {
                        best_match = Some((account, info, price));
                    }
                    _ => {}
                }
            }

            best_match
                .map(|(account, info, _)| (account, info))
                .ok_or(Error::<T>::NoMatchingProvider.into())
        }

        fn finalize_agreement(
            bucket_id: BucketId,
            provider: &T::AccountId,
            agreement: &StorageAgreement<T>,
            action: EndAction,
            is_early: bool,
        ) -> DispatchResult {
            let (to_provider, to_burn) = match action {
                EndAction::Pay => (agreement.payment_locked, Zero::zero()),
                EndAction::Burn { burn_percent } => {
                    let burn_percent = burn_percent.min(100);
                    let burn_amount =
                        agreement.payment_locked * burn_percent.into() / 100u32.into();
                    let pay_amount = agreement.payment_locked.saturating_sub(burn_amount);
                    (pay_amount, burn_amount)
                }
            };

            // Unreserve from owner
            T::Currency::unreserve(&agreement.owner, agreement.payment_locked);

            // Pay provider
            if !to_provider.is_zero() {
                T::Currency::transfer(
                    &agreement.owner,
                    provider,
                    to_provider,
                    ExistenceRequirement::KeepAlive,
                )?;
            }

            // Send burned amount to treasury
            if !to_burn.is_zero() {
                T::Currency::transfer(
                    &agreement.owner,
                    &T::Treasury::get(),
                    to_burn,
                    ExistenceRequirement::KeepAlive,
                )?;
            }

            // Update provider stats
            Providers::<T>::mutate(provider, |maybe_provider| {
                if let Some(provider_info) = maybe_provider {
                    provider_info.committed_bytes = provider_info
                        .committed_bytes
                        .saturating_sub(agreement.max_bytes);

                    if to_burn > Zero::zero() {
                        provider_info.stats.agreements_burned =
                            provider_info.stats.agreements_burned.saturating_add(1);
                    } else {
                        provider_info.stats.agreements_not_extended = provider_info
                            .stats
                            .agreements_not_extended
                            .saturating_add(1);
                    }
                }
            });

            // Remove from primary_providers if primary
            if matches!(agreement.role, ProviderRole::Primary) {
                Buckets::<T>::mutate(bucket_id, |maybe_bucket| {
                    if let Some(bucket) = maybe_bucket {
                        bucket.primary_providers.retain(|p| p != provider);
                    }
                });

                let reason = if is_early {
                    RemovalReason::AdminTerminated
                } else {
                    RemovalReason::Expired
                };

                Self::deposit_event(Event::PrimaryProviderRemoved {
                    bucket_id,
                    provider: provider.clone(),
                    reason,
                });
            }

            // Remove agreement
            StorageAgreements::<T>::remove(bucket_id, provider);

            Self::deposit_event(Event::AgreementEnded {
                bucket_id,
                provider: provider.clone(),
                payment_to_provider: to_provider,
                burned: to_burn,
            });

            Ok(())
        }

        /// Internal function to cleanup a bucket and all its agreements.
        /// This is called by Layer 1 (drive-registry) when deleting a drive.
        ///
        /// Returns the total amount refunded to the owner.
        pub fn cleanup_bucket_internal(
            bucket_id: BucketId,
            owner: &T::AccountId,
        ) -> Result<BalanceOf<T>, DispatchError> {
            // Verify bucket exists
            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            // Verify caller is an admin of the bucket
            Self::ensure_admin(owner, &bucket)?;

            let mut total_refunded: BalanceOf<T> = Zero::zero();

            // End all agreements for this bucket (pay providers fairly)
            let agreements: Vec<_> = StorageAgreements::<T>::iter_prefix(bucket_id).collect();

            for (provider, agreement) in agreements {
                // Calculate prorated refund based on remaining time
                let current_block = frame_system::Pallet::<T>::block_number();
                let remaining_blocks = agreement.expires_at.saturating_sub(current_block);

                // If there's remaining time, calculate prorated refund
                let refund_to_owner = if remaining_blocks > Zero::zero() {
                    let total_duration = agreement.expires_at.saturating_sub(agreement.started_at);
                    if total_duration > Zero::zero() {
                        use sp_runtime::traits::SaturatedConversion;
                        let remaining_u128: u128 = remaining_blocks.saturated_into();
                        let total_u128: u128 = total_duration.saturated_into();
                        let payment_u128: u128 = agreement.payment_locked.saturated_into();

                        // refund = payment * (remaining / total)
                        let refund_u128 = payment_u128
                            .saturating_mul(remaining_u128)
                            .saturating_div(total_u128);
                        refund_u128.saturated_into()
                    } else {
                        Zero::zero()
                    }
                } else {
                    Zero::zero()
                };

                // Payment to provider = total locked - refund to owner
                let payment_to_provider = agreement.payment_locked.saturating_sub(refund_to_owner);

                // Unreserve from owner
                T::Currency::unreserve(&agreement.owner, agreement.payment_locked);

                // Pay provider their earned portion
                if !payment_to_provider.is_zero() {
                    T::Currency::transfer(
                        &agreement.owner,
                        &provider,
                        payment_to_provider,
                        ExistenceRequirement::KeepAlive,
                    )?;
                }

                // Track total refunded (owner keeps the unspent portion)
                total_refunded = total_refunded.saturating_add(refund_to_owner);

                // Update provider stats
                Providers::<T>::mutate(&provider, |maybe_provider| {
                    if let Some(provider_info) = maybe_provider {
                        provider_info.committed_bytes = provider_info
                            .committed_bytes
                            .saturating_sub(agreement.max_bytes);
                        provider_info.stats.agreements_not_extended = provider_info
                            .stats
                            .agreements_not_extended
                            .saturating_add(1);
                    }
                });

                // Remove agreement
                StorageAgreements::<T>::remove(bucket_id, &provider);

                Self::deposit_event(Event::AgreementEnded {
                    bucket_id,
                    provider: provider.clone(),
                    payment_to_provider,
                    burned: Zero::zero(),
                });
            }

            // Clean up reverse index for all members
            for member in &bucket.members {
                MemberBuckets::<T>::mutate(&member.account, |buckets| {
                    buckets.retain(|id| *id != bucket_id);
                });
            }

            // Drain any still-pending AgreementRequests for this bucket and
            // refund the requesters' locked funds. Without this the entries
            // outlive the bucket and the provider's auto-coordinator keeps
            // trying to accept them, every accept reverting with
            // BucketNotFound and jamming the coordinator's queue.
            for (provider, request) in AgreementRequests::<T>::drain_prefix(bucket_id) {
                let total_locked = if let Some(ref replica_params) = request.replica_params {
                    request
                        .payment_locked
                        .checked_add(&replica_params.sync_balance)
                        .unwrap_or(request.payment_locked)
                } else {
                    request.payment_locked
                };
                T::Currency::unreserve(&request.requester, total_locked);
                Self::deposit_event(Event::AgreementRequestWithdrawn {
                    bucket_id,
                    provider,
                    payment_returned: total_locked,
                });
            }

            // Remove the bucket itself
            Buckets::<T>::remove(bucket_id);

            Self::deposit_event(Event::BucketDeleted { bucket_id });

            Ok(total_refunded)
        }

        fn create_challenge(
            challenger: T::AccountId,
            bucket_id: BucketId,
            provider: T::AccountId,
            mmr_root: H256,
            start_seq: u64,
            leaf_index: u64,
            chunk_index: u64,
        ) -> DispatchResult {
            // Calculate deposit (simplified - would be based on expected costs)
            let deposit: BalanceOf<T> = 100u32.into();

            T::Currency::reserve(&challenger, deposit)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            let deadline = current_block.saturating_add(T::ChallengeTimeout::get());

            let challenge = Challenge {
                bucket_id,
                provider: provider.clone(),
                challenger: challenger.clone(),
                mmr_root,
                start_seq,
                leaf_index,
                chunk_index,
                deposit,
            };

            let index = Challenges::<T>::mutate(deadline, |challenges| {
                let challenges = challenges.get_or_insert_with(Vec::new);
                let idx = challenges.len() as u16;
                challenges.push(challenge);
                idx
            });

            // Update provider stats
            Providers::<T>::mutate(&provider, |maybe_provider| {
                if let Some(provider_info) = maybe_provider {
                    provider_info.stats.challenges_received =
                        provider_info.stats.challenges_received.saturating_add(1);
                }
            });

            let challenge_id = ChallengeId { deadline, index };

            Self::deposit_event(Event::ChallengeCreated {
                challenge_id,
                bucket_id,
                provider,
                challenger,
                respond_by: deadline,
            });

            Ok(())
        }

        fn update_historical_roots(
            bucket: &mut Bucket<T>,
            current_block: BlockNumberFor<T>,
            mmr_root: H256,
        ) {
            let block_num: u32 = current_block.try_into().unwrap_or(0u32);

            for (i, &prime) in HISTORICAL_ROOT_PRIMES.iter().enumerate() {
                let quotient = block_num / prime;
                if quotient != bucket.historical_roots[i].0 {
                    bucket.historical_roots[i] = (quotient, mmr_root);
                }
            }
        }

        fn find_matching_root(
            bucket: &Bucket<T>,
            roots: &[Option<H256>; 7],
        ) -> Result<(u8, H256), DispatchError> {
            // Check current snapshot first
            if let (Some(snapshot), Some(root)) = (&bucket.snapshot, roots[0]) {
                if snapshot.mmr_root == root {
                    return Ok((0, root));
                }
            }

            // Check historical roots
            for i in 0..6 {
                if let Some(root) = roots[i + 1] {
                    if bucket.historical_roots[i].1 == root {
                        return Ok((i as u8 + 1, root));
                    }
                }
            }

            Err(Error::<T>::InvalidSyncRoot.into())
        }

        /// Slash a provider who failed to respond to a challenge.
        ///
        /// This:
        /// 1. Slashes the provider's entire stake
        /// 2. Refunds the challenger with their deposit plus a reward
        /// 3. Updates provider statistics
        /// 4. Marks the provider as slashed (so they can be removed from buckets)
        fn slash_provider_for_failed_challenge(
            challenge: &Challenge<T>,
            challenge_id: ChallengeId<BlockNumberFor<T>>,
        ) {
            // Get provider info
            if let Some(mut provider_info) = Providers::<T>::get(&challenge.provider) {
                // Slash the provider's entire stake
                let slashed_amount = provider_info.stake;

                // Unreserve and slash the stake
                // In Substrate, slashing typically burns or sends to treasury
                let (_, remaining) =
                    T::Currency::slash_reserved(&challenge.provider, slashed_amount);
                let actually_slashed = slashed_amount.saturating_sub(remaining);

                // Calculate challenger reward (e.g., 10% of slashed amount, rest goes to treasury)
                let challenger_reward = actually_slashed / 10u32.into();
                let to_treasury = actually_slashed.saturating_sub(challenger_reward);

                // Refund challenger's deposit
                T::Currency::unreserve(&challenge.challenger, challenge.deposit);

                // Transfer reward to challenger
                // Note: We need to handle potential errors gracefully in on_finalize
                let _ = T::Currency::deposit_creating(&challenge.challenger, challenger_reward);

                // The rest goes to treasury (burned by slash_reserved)
                let _ = to_treasury; // Acknowledged

                // Update provider stats
                provider_info.stats.challenges_failed =
                    provider_info.stats.challenges_failed.saturating_add(1);
                provider_info.stake = Zero::zero();

                Providers::<T>::insert(&challenge.provider, provider_info);

                // Emit event
                Self::deposit_event(Event::ChallengeSlashed {
                    challenge_id,
                    provider: challenge.provider.clone(),
                    slashed_amount: actually_slashed,
                    challenger_reward,
                });
            }
        }

        // ─────────────────────────────────────────────────────────────────────────
        // Provider-Initiated Checkpoint Helpers
        // ─────────────────────────────────────────────────────────────────────────

        /// Calculate the checkpoint window number for a given block.
        ///
        /// Window 0 starts at block 0, window 1 at block `interval`, etc.
        fn calculate_window(block: BlockNumberFor<T>, interval: BlockNumberFor<T>) -> u64 {
            use sp_runtime::traits::SaturatedConversion;
            if interval.is_zero() {
                return 0;
            }
            let block_num: u64 = block.saturated_into();
            let interval_num: u64 = interval.saturated_into();
            block_num / interval_num
        }

        /// Calculate the start block for a given checkpoint window.
        fn window_start_block(window: u64, interval: BlockNumberFor<T>) -> BlockNumberFor<T> {
            use sp_runtime::traits::SaturatedConversion;
            let interval_num: u64 = interval.saturated_into();
            let start: u64 = window.saturating_mul(interval_num);
            start.saturated_into()
        }

        /// Calculate the leader index for a given bucket and window.
        ///
        /// Uses deterministic selection: blake2_256(bucket_id || window) % num_providers.
        /// This ensures all providers can independently calculate who the leader is.
        fn calculate_leader_index(bucket_id: BucketId, window: u64, num_providers: u32) -> u32 {
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
        fn get_checkpoint_config(
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

        /// Check if the current block is within the grace period for a window.
        fn is_within_grace_period(
            current_block: BlockNumberFor<T>,
            window: u64,
            config: &storage_primitives::CheckpointWindowConfig<BlockNumberFor<T>>,
        ) -> bool {
            let window_start = Self::window_start_block(window, config.interval);
            let grace_end = window_start.saturating_add(config.grace_period);
            current_block <= grace_end
        }

        // ─────────────────────────────────────────────────────────────────────────
        // Runtime API Implementation
        // ─────────────────────────────────────────────────────────────────────────

        /// Query provider information.
        pub fn query_provider_info(
            provider: &T::AccountId,
        ) -> Option<crate::runtime_api::ProviderInfoResponse> {
            use sp_runtime::traits::SaturatedConversion;

            Providers::<T>::get(provider).map(|info| {
                let max_capacity = info.settings.max_capacity;
                let available_capacity = if max_capacity > 0 {
                    Some(max_capacity.saturating_sub(info.committed_bytes))
                } else {
                    None // Unlimited
                };

                crate::runtime_api::ProviderInfoResponse {
                    multiaddr: info.multiaddr.to_vec(),
                    public_key: info.public_key.to_vec(),
                    stake: info.stake.saturated_into::<u128>(),
                    committed_bytes: info.committed_bytes,
                    min_duration: info.settings.min_duration.saturated_into::<u32>(),
                    max_duration: info.settings.max_duration.saturated_into::<u32>(),
                    price_per_byte: info.settings.price_per_byte.saturated_into::<u128>(),
                    accepting_primary: info.settings.accepting_primary,
                    replica_sync_price: info
                        .settings
                        .replica_sync_price
                        .map(|p| p.saturated_into::<u128>()),
                    accepting_extensions: info.settings.accepting_extensions,
                    registered_at: info.stats.registered_at.saturated_into::<u32>(),
                    agreements_total: info.stats.agreements_total,
                    agreements_extended: info.stats.agreements_extended,
                    agreements_not_extended: info.stats.agreements_not_extended,
                    agreements_burned: info.stats.agreements_burned,
                    challenges_received: info.stats.challenges_received,
                    challenges_failed: info.stats.challenges_failed,
                    max_capacity,
                    available_capacity,
                }
            })
        }

        /// Query all providers (paginated).
        pub fn query_providers(
            offset: u32,
            limit: u32,
        ) -> Vec<(T::AccountId, crate::runtime_api::ProviderInfoResponse)> {
            use sp_runtime::traits::SaturatedConversion;

            Providers::<T>::iter()
                .skip(offset as usize)
                .take(limit as usize)
                .map(|(account, info)| {
                    let max_capacity = info.settings.max_capacity;
                    let available_capacity = if max_capacity > 0 {
                        Some(max_capacity.saturating_sub(info.committed_bytes))
                    } else {
                        None // Unlimited
                    };

                    (
                        account,
                        crate::runtime_api::ProviderInfoResponse {
                            multiaddr: info.multiaddr.to_vec(),
                            public_key: info.public_key.to_vec(),
                            stake: info.stake.saturated_into::<u128>(),
                            committed_bytes: info.committed_bytes,
                            min_duration: info.settings.min_duration.saturated_into::<u32>(),
                            max_duration: info.settings.max_duration.saturated_into::<u32>(),
                            price_per_byte: info.settings.price_per_byte.saturated_into::<u128>(),
                            accepting_primary: info.settings.accepting_primary,
                            replica_sync_price: info
                                .settings
                                .replica_sync_price
                                .map(|p| p.saturated_into::<u128>()),
                            accepting_extensions: info.settings.accepting_extensions,
                            registered_at: info.stats.registered_at.saturated_into::<u32>(),
                            agreements_total: info.stats.agreements_total,
                            agreements_extended: info.stats.agreements_extended,
                            agreements_not_extended: info.stats.agreements_not_extended,
                            agreements_burned: info.stats.agreements_burned,
                            challenges_received: info.stats.challenges_received,
                            challenges_failed: info.stats.challenges_failed,
                            max_capacity,
                            available_capacity,
                        },
                    )
                })
                .collect()
        }

        /// Query bucket information.
        pub fn query_bucket_info(
            bucket_id: BucketId,
        ) -> Option<crate::runtime_api::BucketResponse> {
            use sp_runtime::traits::SaturatedConversion;

            Buckets::<T>::get(bucket_id).map(|bucket| crate::runtime_api::BucketResponse {
                bucket_id,
                members: bucket
                    .members
                    .iter()
                    .map(|m| crate::runtime_api::BucketMemberResponse {
                        account: m.account.encode(),
                        role: m.role,
                    })
                    .collect(),
                frozen_start_seq: bucket.frozen_start_seq,
                min_providers: bucket.min_providers,
                primary_providers: bucket
                    .primary_providers
                    .iter()
                    .map(|p| p.encode())
                    .collect(),
                snapshot: bucket.snapshot.map(|s| BucketSnapshot {
                    mmr_root: s.mmr_root,
                    start_seq: s.start_seq,
                    leaf_count: s.leaf_count,
                    checkpoint_block: s.checkpoint_block.saturated_into::<u32>(),
                    primary_signers: s.primary_signers.clone(),
                }),
                total_snapshots: bucket.total_snapshots,
            })
        }

        /// Query bucket providers.
        pub fn query_bucket_providers(bucket_id: BucketId) -> Vec<T::AccountId> {
            Buckets::<T>::get(bucket_id)
                .map(|bucket| bucket.primary_providers.to_vec())
                .unwrap_or_default()
        }

        /// Query agreement information.
        pub fn query_agreement_info(
            bucket_id: BucketId,
            provider: &T::AccountId,
        ) -> Option<crate::runtime_api::AgreementResponse> {
            use sp_runtime::traits::SaturatedConversion;

            StorageAgreements::<T>::get(bucket_id, provider).map(|agreement| {
                crate::runtime_api::AgreementResponse {
                    owner: agreement.owner.encode(),
                    provider: provider.encode(),
                    max_bytes: agreement.max_bytes,
                    payment_locked: agreement.payment_locked.saturated_into::<u128>(),
                    price_per_byte: agreement.price_per_byte.saturated_into::<u128>(),
                    expires_at: agreement.expires_at.saturated_into::<u32>(),
                    extensions_blocked: agreement.extensions_blocked,
                    role: match agreement.role {
                        ProviderRole::Primary => ProviderRole::Primary,
                        ProviderRole::Replica {
                            sync_balance,
                            sync_price,
                            min_sync_interval,
                            last_sync,
                        } => ProviderRole::Replica {
                            sync_balance: sync_balance.saturated_into::<u128>(),
                            sync_price: sync_price.saturated_into::<u128>(),
                            min_sync_interval: min_sync_interval.saturated_into::<u32>(),
                            last_sync: last_sync
                                .map(|(root, block)| (root, block.saturated_into::<u32>())),
                        },
                    },
                    started_at: agreement.started_at.saturated_into::<u32>(),
                }
            })
        }

        /// Query all agreements for a bucket.
        pub fn query_bucket_agreements(
            bucket_id: BucketId,
        ) -> Vec<crate::runtime_api::AgreementResponse> {
            use sp_runtime::traits::SaturatedConversion;

            StorageAgreements::<T>::iter_prefix(bucket_id)
                .map(
                    |(provider, agreement)| crate::runtime_api::AgreementResponse {
                        owner: agreement.owner.encode(),
                        provider: provider.encode(),
                        max_bytes: agreement.max_bytes,
                        payment_locked: agreement.payment_locked.saturated_into::<u128>(),
                        price_per_byte: agreement.price_per_byte.saturated_into::<u128>(),
                        expires_at: agreement.expires_at.saturated_into::<u32>(),
                        extensions_blocked: agreement.extensions_blocked,
                        role: match agreement.role {
                            ProviderRole::Primary => ProviderRole::Primary,
                            ProviderRole::Replica {
                                sync_balance,
                                sync_price,
                                min_sync_interval,
                                last_sync,
                            } => ProviderRole::Replica {
                                sync_balance: sync_balance.saturated_into::<u128>(),
                                sync_price: sync_price.saturated_into::<u128>(),
                                min_sync_interval: min_sync_interval.saturated_into::<u32>(),
                                last_sync: last_sync
                                    .map(|(root, block)| (root, block.saturated_into::<u32>())),
                            },
                        },
                        started_at: agreement.started_at.saturated_into::<u32>(),
                    },
                )
                .collect()
        }

        /// Query all bucket IDs (paginated).
        pub fn query_bucket_ids(offset: u32, limit: u32) -> Vec<BucketId> {
            Buckets::<T>::iter_keys()
                .skip(offset as usize)
                .take(limit as usize)
                .collect()
        }

        /// Query all agreements for a provider.
        pub fn query_provider_agreements(
            provider: &T::AccountId,
        ) -> Vec<crate::runtime_api::AgreementResponse> {
            use sp_runtime::traits::SaturatedConversion;

            StorageAgreements::<T>::iter()
                .filter(|(_, p, _)| p == provider)
                .map(
                    |(_bucket_id, _, agreement)| crate::runtime_api::AgreementResponse {
                        owner: agreement.owner.encode(),
                        provider: provider.encode(),
                        max_bytes: agreement.max_bytes,
                        payment_locked: agreement.payment_locked.saturated_into::<u128>(),
                        price_per_byte: agreement.price_per_byte.saturated_into::<u128>(),
                        expires_at: agreement.expires_at.saturated_into::<u32>(),
                        extensions_blocked: agreement.extensions_blocked,
                        role: match agreement.role {
                            ProviderRole::Primary => ProviderRole::Primary,
                            ProviderRole::Replica {
                                sync_balance,
                                sync_price,
                                min_sync_interval,
                                last_sync,
                            } => ProviderRole::Replica {
                                sync_balance: sync_balance.saturated_into::<u128>(),
                                sync_price: sync_price.saturated_into::<u128>(),
                                min_sync_interval: min_sync_interval.saturated_into::<u32>(),
                                last_sync: last_sync
                                    .map(|(root, block)| (root, block.saturated_into::<u32>())),
                            },
                        },
                        started_at: agreement.started_at.saturated_into::<u32>(),
                    },
                )
                .collect()
        }

        /// Query challenges expiring at a specific block.
        pub fn query_challenges_at(
            block: BlockNumberFor<T>,
        ) -> Vec<crate::runtime_api::ChallengeResponse> {
            use sp_runtime::traits::SaturatedConversion;

            Challenges::<T>::get(block)
                .unwrap_or_default()
                .iter()
                .map(|challenge| crate::runtime_api::ChallengeResponse {
                    bucket_id: challenge.bucket_id,
                    provider: challenge.provider.encode(),
                    challenger: challenge.challenger.encode(),
                    mmr_root: challenge.mmr_root,
                    start_seq: challenge.start_seq,
                    leaf_index: challenge.leaf_index,
                    chunk_index: challenge.chunk_index,
                    deadline: block.saturated_into::<u32>(),
                    deposit: challenge.deposit.saturated_into::<u128>(),
                })
                .collect()
        }

        /// Check if provider can accept additional bytes.
        pub fn query_can_accept_bytes(provider: &T::AccountId, additional_bytes: u64) -> bool {
            use sp_runtime::traits::SaturatedConversion;

            if let Some(provider_info) = Providers::<T>::get(provider) {
                let new_committed_bytes = provider_info
                    .committed_bytes
                    .saturating_add(additional_bytes);

                // Check capacity constraint
                if provider_info.settings.max_capacity > 0
                    && new_committed_bytes > provider_info.settings.max_capacity
                {
                    return false;
                }

                let bytes_as_balance: BalanceOf<T> = new_committed_bytes.saturated_into();

                if let Some(required_stake) =
                    T::MinStakePerByte::get().checked_mul(&bytes_as_balance)
                {
                    return provider_info.stake >= required_stake;
                }
            }
            false
        }

        // ─────────────────────────────────────────────────────────────────────────
        // Internal Functions for Inter-Pallet Communication (Layer 1 File System)
        // ─────────────────────────────────────────────────────────────────────────

        /// Create a bucket internally (for use by other pallets like Layer 1 File System).
        ///
        /// This bypasses the normal extrinsic flow and creates a bucket directly,
        /// with the specified account as admin.
        ///
        /// Parameters:
        /// - `admin`: Account that will be the bucket admin
        /// - `min_providers`: Minimum number of providers required
        ///
        /// Returns: bucket_id
        pub fn create_bucket_internal(
            admin: &T::AccountId,
            min_providers: u32,
        ) -> Result<BucketId, DispatchError> {
            let bucket_id = NextBucketId::<T>::get();
            NextBucketId::<T>::put(bucket_id.saturating_add(1));

            let admin_member = Member {
                account: admin.clone(),
                role: Role::Admin,
            };

            let mut members = BoundedVec::new();
            members
                .try_push(admin_member)
                .map_err(|_| Error::<T>::MaxMembersReached)?;

            let bucket = Bucket {
                members,
                frozen_start_seq: None,
                min_providers,
                primary_providers: BoundedVec::new(),
                snapshot: None,
                historical_roots: [(0, H256::zero()); 6],
                total_snapshots: 0,
            };

            Buckets::<T>::insert(bucket_id, bucket);

            // Update reverse index for creator
            MemberBuckets::<T>::try_mutate(admin, |buckets| {
                buckets
                    .try_push(bucket_id)
                    .map_err(|_| Error::<T>::TooManyBucketsForMember)
            })?;

            Self::deposit_event(Event::BucketCreated {
                bucket_id,
                admin: admin.clone(),
            });

            Ok(bucket_id)
        }

        /// Request a primary storage agreement internally (for use by other pallets).
        ///
        /// This creates a primary storage agreement without requiring admin origin check.
        ///
        /// Parameters:
        /// - `owner`: Account that owns the agreement and will pay for it
        /// - `bucket_id`: Target bucket
        /// - `provider`: Provider to store data
        /// - `max_bytes`: Maximum storage size
        /// - `duration`: Storage duration in blocks
        /// - `max_payment`: Maximum payment willing to pay
        pub fn request_primary_agreement_internal(
            owner: &T::AccountId,
            bucket_id: BucketId,
            provider: &T::AccountId,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
            max_payment: BalanceOf<T>,
        ) -> DispatchResult {
            let bucket = Buckets::<T>::get(bucket_id).ok_or(Error::<T>::BucketNotFound)?;

            // Check primary provider limit
            ensure!(
                bucket.primary_providers.len() < T::MaxPrimaryProviders::get() as usize,
                Error::<T>::MaxPrimaryProvidersReached
            );

            let provider_info =
                Providers::<T>::get(provider).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            ensure!(
                provider_info.settings.accepting_primary,
                Error::<T>::ProviderNotAcceptingPrimary
            );

            Self::validate_duration(&provider_info.settings, duration)?;

            let payment = Self::calculate_payment(
                provider_info.settings.price_per_byte,
                max_bytes,
                duration,
            )?;
            ensure!(payment <= max_payment, Error::<T>::PaymentExceedsMax);

            T::Currency::reserve(owner, payment)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            let expires_at = current_block.saturating_add(T::RequestTimeout::get());

            let request = AgreementRequest {
                requester: owner.clone(),
                max_bytes,
                payment_locked: payment,
                duration,
                expires_at,
                replica_params: None, // Primary agreement
            };

            ensure!(
                !AgreementRequests::<T>::contains_key(bucket_id, provider),
                Error::<T>::AgreementRequestAlreadyExists
            );

            AgreementRequests::<T>::insert(bucket_id, provider, request);

            Self::deposit_event(Event::AgreementRequested {
                bucket_id,
                provider: provider.clone(),
                requester: owner.clone(),
                max_bytes,
                payment_locked: payment,
                duration,
            });

            Ok(())
        }

        /// Request a replica storage agreement internally (for use by other pallets).
        ///
        /// This creates a replica storage agreement without requiring origin check.
        ///
        /// Parameters:
        /// - `owner`: Account that owns the agreement and will pay for it
        /// - `bucket_id`: Target bucket
        /// - `provider`: Provider to store replica
        /// - `max_bytes`: Maximum storage size
        /// - `duration`: Storage duration in blocks
        /// - `max_payment`: Maximum payment willing to pay
        /// - `sync_balance`: Balance reserved for sync operations
        pub fn request_replica_agreement_internal(
            owner: &T::AccountId,
            bucket_id: BucketId,
            provider: &T::AccountId,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
            max_payment: BalanceOf<T>,
            sync_balance: BalanceOf<T>,
        ) -> DispatchResult {
            ensure!(
                Buckets::<T>::contains_key(bucket_id),
                Error::<T>::BucketNotFound
            );

            let provider_info =
                Providers::<T>::get(provider).ok_or(Error::<T>::ProviderNotFound)?;
            Self::ensure_provider_active(&provider_info)?;

            ensure!(
                provider_info.settings.replica_sync_price.is_some(),
                Error::<T>::ProviderNotAcceptingReplicas
            );

            Self::validate_duration(&provider_info.settings, duration)?;

            // Calculate payment
            let payment = Self::calculate_payment(
                provider_info.settings.price_per_byte,
                max_bytes,
                duration,
            )?;
            ensure!(payment <= max_payment, Error::<T>::PaymentExceedsMax);

            // Total to lock = storage payment + sync balance
            let total_lock = payment
                .checked_add(&sync_balance)
                .ok_or(Error::<T>::ArithmeticOverflow)?;

            // Reserve funds
            T::Currency::reserve(owner, total_lock)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            let expires_at = current_block.saturating_add(T::RequestTimeout::get());

            let replica_params = ReplicaRequestParams {
                sync_balance,
                min_sync_interval: duration / 10u32.into(), // Sync every 10% of duration
            };

            let request = AgreementRequest {
                requester: owner.clone(),
                max_bytes,
                payment_locked: payment,
                duration,
                expires_at,
                replica_params: Some(replica_params),
            };

            ensure!(
                !AgreementRequests::<T>::contains_key(bucket_id, provider),
                Error::<T>::AgreementRequestAlreadyExists
            );

            AgreementRequests::<T>::insert(bucket_id, provider, request);

            Self::deposit_event(Event::AgreementRequested {
                bucket_id,
                provider: provider.clone(),
                requester: owner.clone(),
                max_bytes,
                payment_locked: total_lock,
                duration,
            });

            Ok(())
        }

        /// Query available providers that can accept storage of given size
        ///
        /// This is a helper for Layer 1 to find suitable providers automatically.
        ///
        /// Parameters:
        /// - `max_bytes`: Storage size needed
        /// - `accepting_primary`: True to filter for primary providers, false for replica providers
        ///
        /// Returns: Vec of provider account IDs that can accept the storage
        pub fn query_available_providers(
            max_bytes: u64,
            accepting_primary: bool,
        ) -> Vec<T::AccountId> {
            Providers::<T>::iter()
                .filter_map(|(account, info)| {
                    // Check if provider is accepting the right type of agreements
                    let accepts_type = if accepting_primary {
                        info.settings.accepting_primary
                    } else {
                        info.settings.replica_sync_price.is_some()
                    };

                    if !accepts_type {
                        return None;
                    }

                    // Check if provider has capacity
                    if Self::query_can_accept_bytes(&account, max_bytes) {
                        Some(account)
                    } else {
                        None
                    }
                })
                .collect()
        }

        // ─────────────────────────────────────────────────────────────────────────
        // Marketplace Query Functions (Provider Discovery)
        // ─────────────────────────────────────────────────────────────────────────

        /// Find providers matching the given storage requirements.
        pub fn query_find_matching_providers(
            requirements: crate::runtime_api::StorageRequirements,
            limit: u32,
        ) -> Vec<crate::runtime_api::MatchedProvider> {
            use crate::runtime_api::{MatchedProvider, PartialMatchReason};
            use sp_runtime::traits::SaturatedConversion;

            let mut results: Vec<MatchedProvider> = Vec::new();

            for (account, info) in Providers::<T>::iter() {
                let max_capacity = info.settings.max_capacity;
                let available = if max_capacity > 0 {
                    max_capacity.saturating_sub(info.committed_bytes)
                } else {
                    u64::MAX // Unlimited
                };

                let price: u128 = info.settings.price_per_byte.saturated_into();
                let min_dur: u32 = info.settings.min_duration.saturated_into();
                let max_dur: u32 = info.settings.max_duration.saturated_into();

                // Determine match score and partial reason
                let mut score: u8 = 100;
                let mut partial_reason: Option<PartialMatchReason> = None;

                // Check accepting status
                // Primary required: must accept primary
                // Replica acceptable: must accept primary OR have replica sync price
                let not_accepting = if requirements.primary_only {
                    !info.settings.accepting_primary
                } else {
                    !info.settings.accepting_primary && info.settings.replica_sync_price.is_none()
                };
                if not_accepting {
                    score = 0;
                    partial_reason = Some(PartialMatchReason::NotAccepting);
                }

                // Check capacity
                if score > 0 && available < requirements.bytes_needed {
                    score = score.saturating_sub(50);
                    if partial_reason.is_none() {
                        partial_reason = Some(PartialMatchReason::InsufficientCapacity);
                    }
                }

                // Check price
                if score > 0 && price > requirements.max_price_per_byte {
                    score = score.saturating_sub(30);
                    if partial_reason.is_none() {
                        partial_reason = Some(PartialMatchReason::PriceTooHigh);
                    }
                }

                // Check duration
                if score > 0
                    && (requirements.min_duration < min_dur || requirements.min_duration > max_dur)
                {
                    score = score.saturating_sub(20);
                    if partial_reason.is_none() {
                        partial_reason = Some(PartialMatchReason::DurationMismatch);
                    }
                }

                // Build the available_capacity field
                let available_capacity = if max_capacity > 0 {
                    Some(available)
                } else {
                    None
                };

                let provider_response = crate::runtime_api::ProviderInfoResponse {
                    multiaddr: info.multiaddr.to_vec(),
                    public_key: info.public_key.to_vec(),
                    stake: info.stake.saturated_into::<u128>(),
                    committed_bytes: info.committed_bytes,
                    min_duration: min_dur,
                    max_duration: max_dur,
                    price_per_byte: price,
                    accepting_primary: info.settings.accepting_primary,
                    replica_sync_price: info
                        .settings
                        .replica_sync_price
                        .map(|p| p.saturated_into::<u128>()),
                    accepting_extensions: info.settings.accepting_extensions,
                    registered_at: info.stats.registered_at.saturated_into::<u32>(),
                    agreements_total: info.stats.agreements_total,
                    agreements_extended: info.stats.agreements_extended,
                    agreements_not_extended: info.stats.agreements_not_extended,
                    agreements_burned: info.stats.agreements_burned,
                    challenges_received: info.stats.challenges_received,
                    challenges_failed: info.stats.challenges_failed,
                    max_capacity,
                    available_capacity,
                };

                results.push(MatchedProvider {
                    account: account.encode(),
                    info: provider_response,
                    match_score: score,
                    available_capacity,
                    partial_reason,
                });
            }

            // Sort by score descending, then by price ascending for ties
            results.sort_by(|a, b| {
                b.match_score
                    .cmp(&a.match_score)
                    .then(a.info.price_per_byte.cmp(&b.info.price_per_byte))
            });

            results.truncate(limit as usize);
            results
        }

        /// Get providers with sufficient capacity for the given bytes (paginated).
        pub fn query_providers_with_capacity(
            bytes_needed: u64,
            offset: u32,
            limit: u32,
        ) -> Vec<(T::AccountId, crate::runtime_api::ProviderInfoResponse)> {
            use sp_runtime::traits::SaturatedConversion;

            Providers::<T>::iter()
                .filter(|(_, info)| {
                    // Check accepting status
                    if !info.settings.accepting_primary
                        && info.settings.replica_sync_price.is_none()
                    {
                        return false;
                    }

                    // Check capacity
                    let max_capacity = info.settings.max_capacity;
                    if max_capacity > 0 {
                        let available = max_capacity.saturating_sub(info.committed_bytes);
                        if available < bytes_needed {
                            return false;
                        }
                    }

                    // Check stake (can they back the additional bytes?)
                    let new_committed = info.committed_bytes.saturating_add(bytes_needed);
                    let bytes_as_balance: BalanceOf<T> = new_committed.saturated_into();
                    if let Some(required_stake) =
                        T::MinStakePerByte::get().checked_mul(&bytes_as_balance)
                    {
                        return info.stake >= required_stake;
                    }
                    false
                })
                .skip(offset as usize)
                .take(limit as usize)
                .map(|(account, info)| {
                    let max_capacity = info.settings.max_capacity;
                    let available_capacity = if max_capacity > 0 {
                        Some(max_capacity.saturating_sub(info.committed_bytes))
                    } else {
                        None
                    };

                    (
                        account,
                        crate::runtime_api::ProviderInfoResponse {
                            multiaddr: info.multiaddr.to_vec(),
                            public_key: info.public_key.to_vec(),
                            stake: info.stake.saturated_into::<u128>(),
                            committed_bytes: info.committed_bytes,
                            min_duration: info.settings.min_duration.saturated_into::<u32>(),
                            max_duration: info.settings.max_duration.saturated_into::<u32>(),
                            price_per_byte: info.settings.price_per_byte.saturated_into::<u128>(),
                            accepting_primary: info.settings.accepting_primary,
                            replica_sync_price: info
                                .settings
                                .replica_sync_price
                                .map(|p| p.saturated_into::<u128>()),
                            accepting_extensions: info.settings.accepting_extensions,
                            registered_at: info.stats.registered_at.saturated_into::<u32>(),
                            agreements_total: info.stats.agreements_total,
                            agreements_extended: info.stats.agreements_extended,
                            agreements_not_extended: info.stats.agreements_not_extended,
                            agreements_burned: info.stats.agreements_burned,
                            challenges_received: info.stats.challenges_received,
                            challenges_failed: info.stats.challenges_failed,
                            max_capacity,
                            available_capacity,
                        },
                    )
                })
                .collect()
        }
    }
}
