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

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
    use alloc::vec::Vec;
    use bitvec::{order::Lsb0, vec::BitVec};
    use frame_support::{
        pallet_prelude::*,
        traits::{Currency, ExistenceRequirement, ReservableCurrency},
    };
    use frame_system::pallet_prelude::*;
    use sp_core::H256;
    use sp_runtime::traits::{CheckedAdd, CheckedSub, Saturating, Verify, Zero};
    use storage_primitives::{
        BucketId, BucketSnapshot, ChallengeId, CommitmentPayload, EndAction, MerkleProof,
        MmrProof, ProviderRole, RemovalReason, ReplicaRequestParams, Role,
        HISTORICAL_ROOT_PRIMES,
    };

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Currency type for payments and staking.
        type Currency: ReservableCurrency<Self::AccountId>;

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
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Storage Items
    // ─────────────────────────────────────────────────────────────────────────

    /// Provider registry.
    #[pallet::storage]
    #[pallet::getter(fn providers)]
    pub type Providers<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ProviderInfo<T>>;

    /// Monotonically increasing bucket ID counter.
    #[pallet::storage]
    #[pallet::getter(fn next_bucket_id)]
    pub type NextBucketId<T: Config> = StorageValue<_, BucketId, ValueQuery>;

    /// Buckets: containers for data with membership and storage agreements.
    #[pallet::storage]
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

    /// Pending agreement requests (provider → bucket → request).
    #[pallet::storage]
    #[pallet::getter(fn agreement_requests)]
    pub type AgreementRequests<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        Blake2_128Concat,
        BucketId,
        AgreementRequest<T>,
    >;

    /// Pending challenges indexed by deadline block.
    #[pallet::storage]
    #[pallet::getter(fn challenges)]
    pub type Challenges<T: Config> =
        StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, Vec<Challenge<T>>>;

    // ─────────────────────────────────────────────────────────────────────────
    // Types
    // ─────────────────────────────────────────────────────────────────────────

    /// Provider information stored on-chain.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
    #[scale_info(skip_type_params(T))]
    pub struct ProviderInfo<T: Config> {
        /// Multiaddr for connecting to this provider.
        pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
        /// Total stake locked by this provider.
        pub stake: BalanceOf<T>,
        /// Total contracted bytes (sum of max_bytes across all agreements).
        pub committed_bytes: u64,
        /// Provider settings.
        pub settings: ProviderSettings<T>,
        /// Provider statistics.
        pub stats: ProviderStats<T>,
    }

    /// Provider settings controlling pricing and availability.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
            }
        }
    }

    /// On-chain statistics for evaluating provider quality.
    #[derive(
        Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug, Default,
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
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
    #[scale_info(skip_type_params(T))]
    pub struct Member<T: Config> {
        pub account: T::AccountId,
        pub role: Role,
    }

    /// Bucket container for data with membership and storage agreements.
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, RuntimeDebug)]
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
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, RuntimeDebug)]
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
    #[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, RuntimeDebug)]
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
        ProviderStakeAdded {
            provider: T::AccountId,
            amount: BalanceOf<T>,
            total_stake: BalanceOf<T>,
        },
        ProviderSettingsUpdated {
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
        ProviderHasActiveAgreements,
        ProviderNotAcceptingPrimary,
        ProviderNotAcceptingReplicas,
        ProviderNotAcceptingExtensions,

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
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn register_provider(
            origin: OriginFor<T>,
            multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
            stake: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(
                !Providers::<T>::contains_key(&who),
                Error::<T>::ProviderAlreadyRegistered
            );
            ensure!(stake >= T::MinProviderStake::get(), Error::<T>::InsufficientStake);

            // Reserve stake
            T::Currency::reserve(&who, stake)?;

            let current_block = frame_system::Pallet::<T>::block_number();

            let provider_info = ProviderInfo {
                multiaddr,
                stake,
                committed_bytes: 0,
                settings: ProviderSettings::default(),
                stats: ProviderStats {
                    registered_at: current_block,
                    ..Default::default()
                },
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn add_stake(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider.as_mut().ok_or(Error::<T>::ProviderNotFound)?;

                T::Currency::reserve(&who, amount)?;

                provider.stake = provider
                    .stake
                    .checked_add(&amount)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;

                Self::deposit_event(Event::ProviderStakeAdded {
                    provider: who.clone(),
                    amount,
                    total_stake: provider.stake,
                });

                Ok(())
            })
        }

        /// Deregister provider and withdraw stake.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn deregister_provider(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let provider = Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;

            ensure!(
                provider.committed_bytes == 0,
                Error::<T>::ProviderHasActiveAgreements
            );

            // Unreserve stake
            T::Currency::unreserve(&who, provider.stake);

            Providers::<T>::remove(&who);

            Self::deposit_event(Event::ProviderDeregistered {
                provider: who,
                stake_returned: provider.stake,
            });

            Ok(())
        }

        /// Update provider settings.
        #[pallet::call_index(3)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn update_provider_settings(
            origin: OriginFor<T>,
            settings: ProviderSettings<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Providers::<T>::try_mutate(&who, |maybe_provider| -> DispatchResult {
                let provider = maybe_provider.as_mut().ok_or(Error::<T>::ProviderNotFound)?;
                provider.settings = settings;
                Ok(())
            })?;

            Self::deposit_event(Event::ProviderSettingsUpdated { provider: who });

            Ok(())
        }

        /// Block or unblock extensions for a specific bucket.
        #[pallet::call_index(4)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn set_extensions_blocked(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            blocked: bool,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(Providers::<T>::contains_key(&who), Error::<T>::ProviderNotFound);

            StorageAgreements::<T>::try_mutate(
                bucket_id,
                &who,
                |maybe_agreement| -> DispatchResult {
                    let agreement =
                        maybe_agreement.as_mut().ok_or(Error::<T>::AgreementNotFound)?;
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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

            Self::deposit_event(Event::BucketCreated {
                bucket_id,
                admin: who,
            });

            Ok(())
        }

        /// Set minimum providers required for checkpoint.
        #[pallet::call_index(11)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn freeze_bucket(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Buckets::<T>::try_mutate(bucket_id, |maybe_bucket| -> DispatchResult {
                let bucket = maybe_bucket.as_mut().ok_or(Error::<T>::BucketNotFound)?;

                Self::ensure_admin(&who, bucket)?;

                ensure!(bucket.frozen_start_seq.is_none(), Error::<T>::BucketFrozen);

                // Require snapshot with min_providers
                let snapshot = bucket.snapshot.as_ref().ok_or(Error::<T>::NoSnapshot)?;

                let signer_count = snapshot.primary_signers.count_ones();
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
                if bucket.members[member_idx].role == Role::Admin && member != who {
                    return Err(Error::<T>::CannotDemoteAdmin.into());
                }

                bucket.members.remove(member_idx);

                Self::deposit_event(Event::MemberRemoved { bucket_id, member });

                Ok(())
            })
        }

        // ─────────────────────────────────────────────────────────────────────
        // Storage Agreements
        // ─────────────────────────────────────────────────────────────────────

        /// Request a replica storage agreement.
        #[pallet::call_index(20)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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

            ensure!(Buckets::<T>::contains_key(bucket_id), Error::<T>::BucketNotFound);

            let provider_info =
                Providers::<T>::get(&provider).ok_or(Error::<T>::ProviderNotFound)?;

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
                !AgreementRequests::<T>::contains_key(&provider, bucket_id),
                Error::<T>::AgreementRequestAlreadyExists
            );

            AgreementRequests::<T>::insert(&provider, bucket_id, request);

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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
                !AgreementRequests::<T>::contains_key(&provider, bucket_id),
                Error::<T>::AgreementRequestAlreadyExists
            );

            AgreementRequests::<T>::insert(&provider, bucket_id, request);

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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn accept_agreement(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(Providers::<T>::contains_key(&who), Error::<T>::ProviderNotFound);

            let request = AgreementRequests::<T>::take(&who, bucket_id)
                .ok_or(Error::<T>::AgreementRequestNotFound)?;

            let current_block = frame_system::Pallet::<T>::block_number();
            ensure!(current_block <= request.expires_at, Error::<T>::RequestExpired);

            let expires_at = current_block.saturating_add(request.duration);

            // Create the role based on whether replica params exist
            let role = if let Some(replica_params) = request.replica_params {
                let provider_info = Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;
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

            let provider_info =
                Providers::<T>::get(&who).ok_or(Error::<T>::ProviderNotFound)?;

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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn reject_agreement(origin: OriginFor<T>, bucket_id: BucketId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let request = AgreementRequests::<T>::take(&who, bucket_id)
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn withdraw_agreement_request(
            origin: OriginFor<T>,
            bucket_id: BucketId,
            provider: T::AccountId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let request = AgreementRequests::<T>::get(&provider, bucket_id)
                .ok_or(Error::<T>::AgreementRequestNotFound)?;

            ensure!(request.requester == who, Error::<T>::NotAgreementOwner);

            AgreementRequests::<T>::remove(&provider, bucket_id);

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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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

                let settlement_deadline =
                    agreement.expires_at.saturating_add(T::SettlementTimeout::get());
                ensure!(
                    current_block <= settlement_deadline,
                    Error::<T>::SettlementWindowPassed
                );
            }

            Self::finalize_agreement(bucket_id, &provider, &agreement, action, is_early_termination)
        }

        /// Claim payment for expired agreement (provider only).
        #[pallet::call_index(26)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn claim_expired_agreement(
            origin: OriginFor<T>,
            bucket_id: BucketId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let agreement = StorageAgreements::<T>::get(bucket_id, &who)
                .ok_or(Error::<T>::AgreementNotFound)?;

            let current_block = frame_system::Pallet::<T>::block_number();

            ensure!(current_block > agreement.expires_at, Error::<T>::AgreementNotExpired);

            let settlement_deadline =
                agreement.expires_at.saturating_add(T::SettlementTimeout::get());
            ensure!(
                current_block > settlement_deadline,
                Error::<T>::AgreementNotExpired
            );

            // Provider claims - treat as Pay
            Self::finalize_agreement(bucket_id, &who, &agreement, EndAction::Pay, false)
        }

        // ─────────────────────────────────────────────────────────────────────
        // Checkpoints
        // ─────────────────────────────────────────────────────────────────────

        /// Submit a new checkpoint with provider signatures.
        #[pallet::call_index(30)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
                    ensure!(start_seq >= frozen_start, Error::<T>::SnapshotViolatesFrozen);
                }

                // Verify signatures and build signer bitfield
                let payload = CommitmentPayload::new(bucket_id, mmr_root, start_seq, leaf_count);
                let encoded_payload = payload.encode();

                let mut primary_signers = BitVec::<u8, Lsb0>::repeat(
                    false,
                    bucket.primary_providers.len(),
                );
                let mut signing_providers = Vec::new();

                for (signer, signature) in signatures.iter() {
                    // Find signer in primary_providers
                    let idx = bucket
                        .primary_providers
                        .iter()
                        .position(|p| p == signer)
                        .ok_or(Error::<T>::ProviderNotInSnapshot)?;

                    // Verify signature
                    let signer_bytes: &[u8] = signer.encode().as_ref();
                    ensure!(
                        signature.verify(
                            encoded_payload.as_slice(),
                            &T::AccountId::decode(&mut &signer_bytes[..])
                                .map_err(|_| Error::<T>::InvalidSignature)?
                        ),
                        Error::<T>::InvalidSignature
                    );

                    primary_signers.set(idx, true);
                    signing_providers.push(signer.clone());
                }

                // Check min_providers
                ensure!(
                    primary_signers.count_ones() >= bucket.min_providers as usize,
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

        // ─────────────────────────────────────────────────────────────────────
        // Challenges
        // ─────────────────────────────────────────────────────────────────────

        /// Challenge on-chain checkpoint.
        #[pallet::call_index(40)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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

            ensure!(
                snapshot.primary_signers.get(provider_idx).map(|b| *b).unwrap_or(false),
                Error::<T>::ProviderNotInSnapshot
            );

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

        /// Respond to a challenge.
        #[pallet::call_index(41)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn respond_to_challenge(
            origin: OriginFor<T>,
            challenge_id: ChallengeId<BlockNumberFor<T>>,
            response: ChallengeResponse<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let mut challenges = Challenges::<T>::get(challenge_id.deadline)
                .ok_or(Error::<T>::ChallengeNotFound)?;

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
            let bucket = Buckets::<T>::get(challenge.bucket_id).ok_or(Error::<T>::BucketNotFound)?;

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

                    // Note: MMR proof verification would go here
                    // For now, we accept the proof
                }
                ChallengeResponse::Deleted {
                    new_start_seq,
                    admin,
                    admin_signature,
                    ..
                } => {
                    // Verify admin is bucket admin
                    Self::ensure_admin(admin, &bucket)?;

                    // Verify challenged seq is before new start
                    let challenged_seq = challenge.start_seq.saturating_add(challenge.leaf_index);
                    ensure!(
                        challenged_seq < *new_start_seq,
                        Error::<T>::InvalidDeletionProof
                    );

                    // Verify admin signature (simplified - would need proper verification)
                    let _ = admin_signature; // Would verify against deletion commitment
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

            // Challenge defended - calculate costs
            let challenge = challenges.remove(challenge_id.index as usize);

            // Update or remove the challenges list
            if challenges.is_empty() {
                Challenges::<T>::remove(challenge_id.deadline);
            } else {
                Challenges::<T>::insert(challenge_id.deadline, challenges);
            }

            // Return most of deposit to challenger (they pay some cost)
            let challenger_cost = challenge.deposit / 4u32.into(); // 25%
            let refund = challenge.deposit.saturating_sub(challenger_cost);
            T::Currency::unreserve(&challenge.challenger, refund);

            // Provider pays from stake (simplified)
            let provider_cost = challenger_cost / 3u32.into();

            let response_time = current_block.saturating_sub(
                challenge_id.deadline.saturating_sub(T::ChallengeTimeout::get()),
            );

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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
                    let agreement =
                        maybe_agreement.as_mut().ok_or(Error::<T>::AgreementNotFound)?;

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
                        ensure!(
                            current_block >= min_next_block,
                            Error::<T>::SyncTooFrequent
                        );
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
        #[pallet::weight(Weight::from_parts(10_000, 0))]
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
                    let agreement =
                        maybe_agreement.as_mut().ok_or(Error::<T>::AgreementNotFound)?;

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

        fn validate_duration(
            settings: &ProviderSettings<T>,
            duration: BlockNumberFor<T>,
        ) -> DispatchResult {
            ensure!(duration >= settings.min_duration, Error::<T>::DurationTooShort);
            ensure!(duration <= settings.max_duration, Error::<T>::DurationTooLong);
            Ok(())
        }

        fn calculate_payment(
            price_per_byte: BalanceOf<T>,
            max_bytes: u64,
            duration: BlockNumberFor<T>,
        ) -> Result<BalanceOf<T>, DispatchError> {
            // payment = price_per_byte * max_bytes * duration
            let bytes_balance: BalanceOf<T> = max_bytes.into();
            let duration_balance: BalanceOf<T> =
                duration.try_into().map_err(|_| Error::<T>::ArithmeticOverflow)?;

            price_per_byte
                .checked_mul(&bytes_balance)
                .and_then(|p| p.checked_mul(&duration_balance))
                .ok_or(Error::<T>::ArithmeticOverflow.into())
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
                    let burn_amount = agreement.payment_locked * burn_percent.into() / 100u32.into();
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

            // Burn (send to treasury or just don't transfer)
            // In practice, you'd send to treasury

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
                        provider_info.stats.agreements_not_extended =
                            provider_info.stats.agreements_not_extended.saturating_add(1);
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

            let challenge_id = ChallengeId {
                deadline,
                index,
            };

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
            let block_num: u32 = current_block
                .try_into()
                .unwrap_or(0u32);

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
    }
}
