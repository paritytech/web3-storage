// SPDX-License-Identifier: Apache-2.0

//! Types and type aliases that the pallet stores, takes as call arguments,
//! or emits in events.

use crate::Config;
use alloc::vec::Vec;
use bitvec::prelude::{BitVec, Lsb0};
use frame_support::{pallet_prelude::*, traits::fungible::Inspect, DefaultNoBound};
use sp_core::H256;
use sp_runtime::traits::{Bounded, SaturatedConversion, Saturating, Zero};
use storage_primitives::{BucketId, ChunkLocation, Commitment, MmrProof, Role, Visibility};

/// Balance type of the configured currency.
pub type BalanceOf<T> =
    <<T as Config>::Currency as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

/// A storage-deposit ticket ([`Config::StorageDeposit`]); stored in the
/// record it pays for and dropped when the record is removed.
pub type TicketOf<T> = <T as Config>::StorageDeposit;

/// Block number of [`Config::BlockNumberProvider`] (the relay chain in
/// production). The pallet measures every duration, deadline and expiry in
/// this block number. The alias has the same name as `frame_system`'s
/// `BlockNumberFor`, so pallet code that writes `BlockNumberFor<T>` gets this
/// block number and not the parachain block number. Only the hooks use the
/// parachain block number, as
/// [`SystemBlockNumberFor`](crate::SystemBlockNumberFor). The `Config` bound
/// requires both block number types to be the same type, so the compiler does
/// not catch a mix-up.
pub type BlockNumberFor<T> =
    <<T as Config>::BlockNumberProvider as sp_runtime::traits::BlockNumberProvider>::BlockNumber;

/// Provider-signed agreement quote bound to this pallet's account, balance,
/// and block-number types.
///
/// Every call that redeems a quote (`create_bucket_with_primary`,
/// `add_primary_provider`, `add_replica_provider`) verifies the provider's
/// signature, checks and advances the owner's agreement nonce, then
/// runs the provider/capacity/stake checks and holds the payment
/// before it writes the agreement.
pub type AgreementTermsOf<T> = storage_primitives::AgreementTerms<
    <T as frame_system::Config>::AccountId,
    BalanceOf<T>,
    BlockNumberFor<T>,
>;

/// Provider information stored on-chain.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
#[scale_info(skip_type_params(T))]
pub struct ProviderInfo<T: Config> {
    /// Multiaddr for connecting to this provider.
    pub multiaddr: BoundedVec<u8, T::MaxMultiaddrLength>,
    /// Public key for signature verification, as raw bytes. The pallet
    /// accepts Sr25519, Ed25519, ECDSA and Ethereum (keccak ECDSA) keys.
    pub public_key: BoundedVec<u8, ConstU32<64>>,
    /// Stake held on the provider under `HoldReason::ProviderStake`.
    pub stake: BalanceOf<T>,
    /// Total contracted bytes: the sum of `max_bytes` across the provider's
    /// current agreements.
    pub committed_bytes: u64,
    /// Pricing and availability settings.
    pub settings: ProviderSettings<T>,
    /// Agreement and challenge counters.
    pub stats: ProviderStats<T>,
    /// Earliest block at which `complete_deregister` can succeed. It also
    /// requires no agreements and no pending challenges. `None` means no
    /// deregistration is announced. Until `complete_deregister` succeeds, the
    /// provider stays registered, a failed challenge still slashes it, and its
    /// stake stays held.
    pub deregister_at: Option<BlockNumberFor<T>>,
    /// Storage deposit for this record: held on the provider at
    /// registration, separate from the stake, released by
    /// `complete_deregister`.
    pub deposit: TicketOf<T>,
}

/// Provider settings controlling pricing and availability.
#[derive(
    CloneNoBound,
    PartialEqNoBound,
    EqNoBound,
    Encode,
    Decode,
    DecodeWithMemTracking,
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
    /// Minimum duration, in anchor blocks, that the provider accepts for a new
    /// agreement or an `extend_agreement` call.
    pub min_duration: BlockNumberFor<T>,
    /// Maximum duration, in anchor blocks, that the provider accepts for a new
    /// agreement or an `extend_agreement` call.
    pub max_duration: BlockNumberFor<T>,
    /// Price per byte per block for storage.
    pub price_per_byte: BalanceOf<T>,
    /// Whether the provider accepts new primary agreements.
    pub accepting_primary: bool,
    /// Price per confirmed replica sync. `None` means the provider accepts no
    /// replica agreements.
    pub replica_sync_price: Option<BalanceOf<T>>,
    /// Whether the provider accepts extensions of existing agreements.
    pub accepting_extensions: bool,
    /// Maximum storage capacity in bytes. 0 means unlimited. The pallet
    /// rejects a new agreement or a top-up that would raise `committed_bytes`
    /// above a non-zero capacity.
    pub max_capacity: u64,
    /// Smallest agreement size, in bytes, that the provider accepts, for
    /// primary and replica agreements. 0 means no minimum. Must not exceed a
    /// non-zero `max_capacity`.
    pub min_agreement_bytes: u64,
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
            max_capacity: 0, // 0 = unlimited
            min_agreement_bytes: 0,
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
    /// Block at which the provider registered.
    pub registered_at: BlockNumberFor<T>,
    /// Total agreements ever created with this provider.
    pub agreements_total: u32,
    /// Number of `extend_agreement` calls on this provider's agreements, by any
    /// caller.
    pub agreements_extended: u32,
    /// Agreements settled after expiry, claimed with
    /// `claim_expired_agreement`, or ended early with `EndAction::Pay`, with
    /// nothing burned.
    pub agreements_not_extended: u32,
    /// Agreements that ended with part of the payment burned.
    pub agreements_burned: u32,
    /// Sum of `max_bytes` at creation over all agreements ever created with
    /// this provider.
    pub total_bytes_committed: u64,
    /// Challenges from authorized challengers (a bucket member or agreement
    /// owner at challenge creation) that the provider defended. The pallet
    /// counts each answered or timed-out challenge in exactly one of
    /// `challenges_received_authorized`, `challenges_received_public` or
    /// `challenges_failed`. A cancelled challenge counts in none.
    pub challenges_received_authorized: u32,
    /// Challenges from unauthorized challengers that the provider defended.
    pub challenges_received_public: u32,
    /// Challenges that slashed the provider, from authorized and
    /// unauthorized challengers.
    pub challenges_failed: u32,
    /// Total payment this provider received for storage service: agreement
    /// settlements, the elapsed-period payment that `extend_agreement`
    /// settles, and replica sync payments. The pallet never decreases or
    /// resets this value.
    pub lifetime_revenue: BalanceOf<T>,
}

impl<T: Config> ProviderStats<T> {
    /// Successfully defended challenges across both challenger tiers.
    pub fn challenges_defended(&self) -> u32 {
        self.challenges_received_authorized
            .saturating_add(self.challenges_received_public)
    }

    /// A provider's 0–100 reputation from its on-chain challenge record:
    /// the share of resolved challenges it defended. Both counters are
    /// tallied at resolution, so pending challenges never count against a
    /// provider.
    ///
    /// A provider with no resolved challenges scores 100, so
    /// `query_challenge_candidates` with a `max_reputation` of 100 or less
    /// does not return a newly registered provider.
    pub fn reputation(&self) -> u8 {
        let defended = self.challenges_defended();
        let total = defended as u64 + self.challenges_failed as u64;
        if total == 0 {
            return 100;
        }
        ((defended as u64 * 100) / total).min(100) as u8
    }
}

impl<T: Config> From<&ProviderStats<T>> for crate::runtime_api::ProviderStatsInfo {
    fn from(stats: &ProviderStats<T>) -> Self {
        Self {
            registered_at: stats.registered_at.saturated_into::<u32>(),
            agreements_total: stats.agreements_total,
            agreements_extended: stats.agreements_extended,
            agreements_not_extended: stats.agreements_not_extended,
            agreements_burned: stats.agreements_burned,
            total_bytes_committed: stats.total_bytes_committed,
            challenges_received_authorized: stats.challenges_received_authorized,
            challenges_received_public: stats.challenges_received_public,
            challenges_failed: stats.challenges_failed,
            lifetime_revenue: stats.lifetime_revenue.saturated_into::<u128>(),
            reputation: stats.reputation(),
        }
    }
}

/// Bucket member with role.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
#[scale_info(skip_type_params(T))]
pub struct Member<T: Config> {
    /// The member's account.
    pub account: T::AccountId,
    /// Role of the member in the bucket.
    pub role: Role,
    /// The admin that added the member and paid `deposit`. Fixed for the
    /// lifetime of the entry.
    pub depositor: T::AccountId,
    /// Storage deposit for this entry and its `MemberBuckets` entry, held
    /// on `depositor` until the member or the bucket is removed.
    pub deposit: TicketOf<T>,
}

/// A bucket: its members, primary providers, visibility and checkpoint state.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[scale_info(skip_type_params(T))]
pub struct Bucket<T: Config> {
    /// Members who can interact with this bucket.
    pub members: BoundedVec<Member<T>, T::MaxMembers>,
    /// Read visibility (see [`Visibility`]). On-chain, only challenge
    /// creation reads it: `Private` allows only bucket members and owners of
    /// primary agreements to challenge a primary provider.
    pub visibility: Visibility,
    /// `None` until `freeze_bucket`. `Some(seq)` makes the bucket append-only:
    /// every later `checkpoint` must use `start_seq == seq`.
    pub frozen_start_seq: Option<u64>,
    /// Minimum primary provider signatures required for checkpoint.
    pub min_providers: u32,
    /// Primary provider accounts.
    pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
    /// Latest checkpoint. `None` before the first `checkpoint`.
    pub snapshot: Option<BucketSnapshot<BlockNumberFor<T>>>,
    /// Recent snapshot commitments for replica sync validation. Entry `i`
    /// contains `(anchor_block / HISTORICAL_ROOT_PRIMES[i], commitment)`, and
    /// `checkpoint` replaces it when the quotient changes. Each entry
    /// contains the full commitment, so a replica that syncs to one of these
    /// roots records a range that `challenge_replica` can bind a proof to.
    pub historical_roots: [(u32, Commitment); 6],
    /// Total snapshots created for this bucket.
    pub total_snapshots: u32,
    /// The account that created the bucket and paid `deposit`. Fixed at
    /// creation; it gets the deposit back even if it is no longer an admin
    /// or a member.
    pub creator: T::AccountId,
    /// Storage deposit for the bucket record, held on `creator`.
    pub deposit: TicketOf<T>,
}

/// Bucket snapshot representing canonical state at a checkpoint.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct BucketSnapshot<BlockNumber> {
    /// Canonical MMR commitment at this checkpoint (root + covered range).
    pub commitment: Commitment,
    /// Anchor block of the `checkpoint` call that created this snapshot.
    pub checkpoint_block: BlockNumber,
    /// Bit i is set if `primary_providers[i]` signed. The length always equals
    /// `primary_providers.len()`: `checkpoint` builds it, adding a primary
    /// appends an unset bit, removing a primary removes its bit, and
    /// `extend_checkpoint` sets the bit of a late signer.
    pub primary_signers: BitVec<u8, Lsb0>,
}

impl<BlockNumber> BucketSnapshot<BlockNumber> {
    /// End of the canonical range (exclusive).
    pub fn range_end(&self) -> u64 {
        self.commitment.range_end()
    }

    /// Whether `seq` is within this snapshot's range.
    pub fn contains_seq(&self, seq: u64) -> bool {
        self.commitment.contains_seq(seq)
    }
}

/// Challenge identifier combining deadline and index.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
pub struct ChallengeId<BlockNumber> {
    /// Block by which the provider must respond.
    pub deadline: BlockNumber,
    /// Index among the challenges that share this deadline.
    pub index: u16,
}

/// Reason for removing a primary provider from a bucket.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
pub enum RemovalReason {
    /// A failed challenge slashed the provider.
    Slashed,
    /// The bucket admin ended the agreement early.
    AdminTerminated,
    /// The agreement reached `expires_at`.
    Expired,
}

/// Provider role for a specific bucket agreement.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum ProviderRole<Balance, BlockNumber> {
    /// Receives data directly from writers. Only a bucket admin can add a
    /// primary provider. Before `expires_at`, only a bucket admin or
    /// `remove_slashed` can end its agreement.
    Primary,
    /// Syncs data from other providers. Anyone can add a replica. Replicas
    /// do not count toward `min_providers`. Before `expires_at`, only
    /// `remove_slashed` can end a replica agreement. The pallet pays `sync_price`
    /// from `sync_balance` for each confirmed sync.
    Replica {
        /// Balance for per-sync payments, reduced on each confirmed sync.
        sync_balance: Balance,
        /// Price per sync, locked at creation or the last extension.
        sync_price: Balance,
        /// Minimum blocks between sync confirmations for this agreement.
        min_sync_interval: BlockNumber,
        /// Last confirmed sync. `None` if the replica hasn't confirmed sync yet.
        last_sync: Option<ReplicaSyncRecord<BlockNumber>>,
    },
}

/// A replica's last confirmed sync: the commitment and the anchor block.
/// `challenge_replica` targets leaves of this commitment.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplicaSyncRecord<BlockNumber> {
    /// MMR commitment the replica confirmed sync to (root + covered range).
    pub commitment: Commitment,
    /// Anchor (relay-chain) block of the `confirm_replica_sync` call.
    pub block: BlockNumber,
}

/// Storage agreement between bucket and provider.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
#[scale_info(skip_type_params(T))]
pub struct StorageAgreement<T: Config> {
    /// Account that owns the agreement. Only the owner can top up the
    /// agreement, transfer ownership, extend it at a higher price, and settle
    /// it within `SettlementTimeout` after expiry.
    ///
    /// The pallet holds the whole escrow on this account, whoever paid it,
    /// and every settlement draws on it. Reassigning `owner` must also move
    /// the hold.
    pub owner: T::AccountId,
    /// Quota in bytes. `top_up_agreement` raises it.
    pub max_bytes: u64,
    /// Prepaid storage fee, held on `owner` under `HoldReason::AgreementPayment`.
    pub payment_locked: BalanceOf<T>,
    /// Price per byte, locked at creation or the last extension.
    pub price_per_byte: BalanceOf<T>,
    /// Anchor block at which the agreement expires.
    pub expires_at: BlockNumberFor<T>,
    /// Whether the provider blocked extensions of this agreement with
    /// `set_extensions_blocked`.
    pub extensions_blocked: bool,
    /// Provider role for this bucket.
    pub role: ProviderRole<BalanceOf<T>, BlockNumberFor<T>>,
    /// Block at which the current payment period started: agreement creation
    /// or the last extension.
    pub started_at: BlockNumberFor<T>,
    /// The account that opened the agreement and paid `deposit`. Fixed at
    /// creation: a transfer moves `owner`, not the deposit, so `new_owner`
    /// pays nothing.
    pub depositor: T::AccountId,
    /// Storage deposit for this record, held on `depositor` until the
    /// agreement is removed.
    pub deposit: TicketOf<T>,
}

impl<T: Config> StorageAgreement<T> {
    /// Everything this agreement keeps under `AgreementPayment` on
    /// `owner`: the prepaid fee plus, for a replica, the unspent sync
    /// balance.
    pub(crate) fn escrow(&self) -> BalanceOf<T> {
        match &self.role {
            ProviderRole::Replica { sync_balance, .. } => {
                self.payment_locked.saturating_add(*sync_balance)
            }
            ProviderRole::Primary => self.payment_locked,
        }
    }
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
    /// The commitment being challenged; its `leaf_count` binds the proof
    /// to the exact `leaf_index` in `respond_to_challenge`.
    pub commitment: Commitment,
    /// Leaf index and chunk index that the challenge targets.
    pub target: ChunkLocation,
    /// Challenger's deposit, held under `HoldReason::ChallengeDeposit`.
    pub deposit: BalanceOf<T>,
    /// Whether the challenger was a bucket member or agreement owner
    /// (`is_authorized`) at challenge creation. The pallet stores this value
    /// so that membership or agreement changes after creation do not change
    /// the fee split in `respond_to_challenge`.
    pub authorized: bool,
    /// Anchor block at which the challenge was created.
    /// `respond_to_challenge` computes the response time for the fee
    /// split from it.
    pub created_at: BlockNumberFor<T>,
}

/// Challenge response from provider.
#[derive(
    CloneNoBound,
    PartialEqNoBound,
    EqNoBound,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub enum ChallengeResponse<T: Config> {
    /// The challenged chunk with an MMR proof for its leaf and a Merkle proof
    /// for the chunk.
    Proof {
        /// Bytes of the challenged chunk.
        chunk_data: BoundedVec<u8, T::MaxChunkSize>,
        /// Proves the leaf is part of the committed MMR.
        mmr_proof: MmrProof,
        /// Proves the chunk is part of the leaf: the sibling hashes from
        /// the chunk up to `mmr_proof.leaf.data_root`.
        chunk_proof: Vec<H256>,
    },
    /// The challenged `chunk_index` is at or beyond the leaf's chunk count,
    /// so the leaf has no such chunk. Valid when the proof is for the
    /// challenged `leaf_index` and the leaf's chunk count (from
    /// `data_size` and `leaf_version`) is at most the index.
    ChunkOutOfRange {
        /// Proves the leaf is part of the committed MMR.
        mmr_proof: MmrProof,
    },
    /// The bucket admin signed a commitment that starts after the challenged
    /// sequence number.
    /// Primary providers only; a replica gets `InvalidDeletionClaim`.
    Deleted {
        /// Root of the newer commitment that no longer contains the
        /// challenged sequence number.
        new_mmr_root: H256,
        /// Start sequence number of that commitment; must be greater than
        /// the challenged sequence number.
        new_start_seq: u64,
        /// Bucket admin who authorized the deletion.
        admin: T::AccountId,
        /// The admin's signature over the SCALE-encoded
        /// `CommitmentPayload::new(bucket_id, Commitment { mmr_root:
        /// new_mmr_root, start_seq: new_start_seq, leaf_count: 0 })`.
        admin_signature: sp_runtime::MultiSignature,
    },
    /// The bucket's current snapshot has a different root from the challenged
    /// commitment and still covers the challenged sequence number.
    /// Primary providers only; a replica gets `InvalidSupersededClaim`.
    Superseded,
}
