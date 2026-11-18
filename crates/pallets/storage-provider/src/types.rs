// SPDX-License-Identifier: Apache-2.0

//! Storage types and type aliases of the pallet.

use crate::Config;
use alloc::vec::Vec;
use bitvec::prelude::{BitVec, Lsb0};
use frame_support::{
    pallet_prelude::*, traits::fungible::Inspect, CloneNoBound, DebugNoBound, DefaultNoBound,
    EqNoBound, PartialEqNoBound,
};
use sp_core::H256;
use sp_runtime::traits::{Bounded, SaturatedConversion, Saturating, Zero};
use storage_primitives::{BucketId, ChunkLocation, Commitment, MmrProof, Role, Visibility};

/// Balance type of the configured currency.
pub type BalanceOf<T> =
    <<T as Config>::Currency as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

/// A storage-deposit ticket ([`Config::StorageDeposit`]); stored in the
/// record it pays for and dropped when the record is removed.
pub type TicketOf<T> = <T as Config>::StorageDeposit;

/// The anchor clock ([`Config::BlockNumberProvider`], relay chain in
/// production) that every duration, deadline and expiry in this pallet is
/// measured against. Deliberately shadows the `frame_system` name,
/// treasury-style: the habitual spelling yields the correct clock, and the
/// parachain height is only reachable as
/// [`SystemBlockNumberFor`](crate::SystemBlockNumberFor) (hooks only). The
/// `Config` bound pins the two equal.
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
    /// Smallest agreement size, in bytes, that the provider accepts, for
    /// primary and replica agreements. 0 = no minimum. Must not exceed a
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
            max_capacity: 0, // 0 = unlimited (backward compatible)
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
    /// Total payment ever received by this provider for storage service:
    /// agreement settlements, extension payments, and replica sync
    /// payments. Monotonically increasing, never reset by a slash or
    /// anything else. A historical record, not a live balance.
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
    /// Providers with no resolved challenges score 100 — benefit of the
    /// doubt, so a newly registered provider is not immediately
    /// challenge-worthy.
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
    /// Role the member holds in the bucket.
    pub role: Role,
    /// The admin that added the member and paid `deposit`. Fixed for the
    /// lifetime of the entry.
    pub depositor: T::AccountId,
    /// Storage deposit for this entry and its `MemberBuckets` entry, held
    /// on `depositor` until the member or the bucket is removed.
    pub deposit: TicketOf<T>,
}

/// Bucket container for data with membership and storage agreements.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[scale_info(skip_type_params(T))]
pub struct Bucket<T: Config> {
    /// Members who can interact with this bucket.
    pub members: BoundedVec<Member<T>, T::MaxMembers>,
    /// Read visibility (see `Visibility`). On-chain, only challenge
    /// creation reads it: `Private` restricts primary challenges to
    /// members and primary-agreement owners.
    pub visibility: Visibility,
    /// If Some, bucket is append-only from this start_seq.
    pub frozen_start_seq: Option<u64>,
    /// Minimum primary provider signatures required for checkpoint.
    pub min_providers: u32,
    /// Primary provider account IDs (limited to T::MaxPrimaryProviders).
    pub primary_providers: BoundedVec<T::AccountId, T::MaxPrimaryProviders>,
    /// Current canonical state.
    pub snapshot: Option<BucketSnapshot<BlockNumberFor<T>>>,
    /// Recent snapshot commitments for replica sync validation, one per
    /// prime-spaced position (see `update_historical_roots`). Each entry is
    /// the full commitment, so a replica that syncs to one of these roots
    /// records a range `challenge_replica` can bind a proof to.
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
#[derive(Clone, PartialEq, Eq, Encode, Decode, codec::DecodeWithMemTracking, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct BucketSnapshot<BlockNumber> {
    /// Canonical MMR commitment at this checkpoint (root + covered range).
    pub commitment: Commitment,
    /// Block at which checkpointed
    pub checkpoint_block: BlockNumber,
    /// Bit i is set if `primary_providers[i]` signed. The length always equals
    /// `primary_providers.len()`: `checkpoint` builds it, adding a primary
    /// appends an unset bit, removing a primary removes its bit, and
    /// `extend_checkpoint` sets the bit of a late signer.
    pub primary_signers: BitVec<u8, Lsb0>,
}

impl<BlockNumber> BucketSnapshot<BlockNumber> {
    /// Get the canonical range end (exclusive)
    pub fn range_end(&self) -> u64 {
        self.commitment.range_end()
    }

    /// Check if a sequence number is within this snapshot's range
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
    codec::DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeId<BlockNumber> {
    /// Block by which provider must respond
    pub deadline: BlockNumber,
    /// Index within the deadline's challenge list
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
    codec::DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum RemovalReason {
    /// Provider was slashed for failing a challenge
    Slashed,
    /// Admin terminated agreement early
    AdminTerminated,
    /// Agreement expired naturally
    Expired,
}

/// Provider role for a specific bucket agreement.
#[derive(
    Clone,
    PartialEq,
    Eq,
    Encode,
    Decode,
    codec::DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum ProviderRole<Balance, BlockNumber> {
    /// Receives data directly from writers. The bucket admin adds and
    /// removes primary providers.
    Primary,
    /// Syncs data from other providers. Anyone can add a replica. It
    /// runs until expiry and is paid `sync_price` from `sync_balance` for
    /// each confirmed sync.
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
    codec::DecodeWithMemTracking,
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
    /// Who owns this agreement (can top up, transfer ownership).
    ///
    /// The whole escrow is held on *this* account, whoever paid it in, and
    /// every settlement path draws on it — so reassigning `owner` must move
    /// the hold too.
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
    /// Leaf + chunk being challenged.
    pub target: ChunkLocation,
    /// Deposit locked by challenger.
    pub deposit: BalanceOf<T>,
    /// Whether the challenger was authorized (bucket member or agreement
    /// owner, via `is_authorized`) at challenge creation. Snapshotted so
    /// membership/agreement changes between creation and response cannot
    /// alter the fee split applied in `respond_to_challenge`.
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
    codec::DecodeWithMemTracking,
    TypeInfo,
    DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub enum ChallengeResponse<T: Config> {
    /// Provide the chunk with proofs.
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
    /// Data was deleted - show newer commitment without this seq.
    /// Primary providers only; a replica gets `InvalidDeletionClaim`.
    Deleted {
        /// Root of the newer commitment that no longer holds the
        /// challenged sequence.
        new_mmr_root: H256,
        /// Start sequence of that commitment; must be past the
        /// challenged one.
        new_start_seq: u64,
        /// Bucket admin who authorised the deletion.
        admin: T::AccountId,
        /// The admin's signature over the newer commitment.
        admin_signature: sp_runtime::MultiSignature,
    },
    /// Challenged state has been superseded by canonical.
    /// Primary providers only; a replica gets `InvalidSupersededClaim`.
    Superseded,
}
