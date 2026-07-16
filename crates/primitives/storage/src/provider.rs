// SPDX-License-Identifier: Apache-2.0

//! Roles, provider agreement types, and provider lifecycle enums.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::{
    traits::{Bounded, Get, Zero},
    BoundedVec,
};

use crate::Commitment;

/// Role within a bucket determining access permissions.
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Role {
    /// Can modify members, manage settings, delete data (if not frozen)
    Admin,
    /// Can append data
    Writer,
    /// Can read data (for private buckets)
    Reader,
}

/// Provider role for a specific bucket agreement.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ProviderRole<Balance, BlockNumber> {
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
        sync_balance: Balance,
        /// Price per sync locked at creation/last extension
        sync_price: Balance,
        /// Minimum blocks between sync confirmations for this agreement.
        min_sync_interval: BlockNumber,
        /// Last confirmed sync. None if replica hasn't confirmed sync yet.
        last_sync: Option<ReplicaSyncRecord<BlockNumber>>,
    },
}

/// Snapshot metadata captured at a replica's `confirm_replica_sync` so the
/// pallet can later challenge a specific leaf without going back to the
/// historical_roots table (which doesn't store sequence metadata).
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplicaSyncRecord<BlockNumber> {
    /// MMR commitment the replica confirmed sync to (root + covered range).
    pub commitment: Commitment,
    /// Block at which `confirm_replica_sync` was executed.
    pub block: BlockNumber,
}

/// Action to take when ending an agreement.
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EndAction {
    /// Pay provider in full
    Pay,
    /// Burn portion, pay rest (0-100%)
    Burn {
        /// Percentage to burn (0-100)
        burn_percent: u8,
    },
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RemovalReason {
    /// Provider was slashed for failing a challenge
    Slashed,
    /// Admin terminated agreement early
    AdminTerminated,
    /// Agreement expired naturally
    Expired,
}

/// Provider information stored on-chain.
#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
#[scale_info(skip_type_params(MaxMultiaddrLength))]
#[codec(mel_bound())]
pub struct ProviderInfo<
    Balance: Encode + Decode + MaxEncodedLen,
    BlockNumber: Encode + Decode + MaxEncodedLen,
    MaxMultiaddrLength: Get<u32>,
> {
    /// Multiaddr for connecting to this provider.
    pub multiaddr: BoundedVec<u8, MaxMultiaddrLength>,
    /// Public key for signature verification.
    /// Stored as raw bytes to support multiple key types (Sr25519, Ed25519, Ecdsa).
    pub public_key: BoundedVec<u8, ConstU32<64>>,
    /// Total stake locked by this provider.
    pub stake: Balance,
    /// Total contracted bytes (sum of max_bytes across all agreements).
    pub committed_bytes: u64,
    /// Provider settings.
    pub settings: ProviderSettings<Balance, BlockNumber>,
    /// Provider statistics.
    pub stats: ProviderStats<BlockNumber>,
    /// Block at which a previously-announced deregistration becomes
    /// finalisable via `complete_deregister`. `None` means no
    /// announcement is in progress. During the announcement window the
    /// provider is still on-chain and still slashable for any pending
    /// challenge — they only get their stake back after the window.
    pub deregister_at: Option<BlockNumber>,
}

// Manual impls instead of derives: std derives would put `Clone`/`PartialEq`/
// `Debug` bounds on the `MaxMultiaddrLength` marker, which `Get<u32>` types
// (e.g. `parameter_types!` structs) do not implement.
impl<Balance, BlockNumber, MaxMultiaddrLength> Clone
    for ProviderInfo<Balance, BlockNumber, MaxMultiaddrLength>
where
    Balance: Encode + Decode + MaxEncodedLen + Clone,
    BlockNumber: Encode + Decode + MaxEncodedLen + Clone,
    MaxMultiaddrLength: Get<u32>,
{
    fn clone(&self) -> Self {
        Self {
            multiaddr: self.multiaddr.clone(),
            public_key: self.public_key.clone(),
            stake: self.stake.clone(),
            committed_bytes: self.committed_bytes,
            settings: self.settings.clone(),
            stats: self.stats.clone(),
            deregister_at: self.deregister_at.clone(),
        }
    }
}

impl<Balance, BlockNumber, MaxMultiaddrLength> PartialEq
    for ProviderInfo<Balance, BlockNumber, MaxMultiaddrLength>
where
    Balance: Encode + Decode + MaxEncodedLen + PartialEq,
    BlockNumber: Encode + Decode + MaxEncodedLen + PartialEq,
    MaxMultiaddrLength: Get<u32>,
{
    fn eq(&self, other: &Self) -> bool {
        self.multiaddr == other.multiaddr
            && self.public_key == other.public_key
            && self.stake == other.stake
            && self.committed_bytes == other.committed_bytes
            && self.settings == other.settings
            && self.stats == other.stats
            && self.deregister_at == other.deregister_at
    }
}

impl<Balance, BlockNumber, MaxMultiaddrLength> Eq
    for ProviderInfo<Balance, BlockNumber, MaxMultiaddrLength>
where
    Balance: Encode + Decode + MaxEncodedLen + Eq,
    BlockNumber: Encode + Decode + MaxEncodedLen + Eq,
    MaxMultiaddrLength: Get<u32>,
{
}

impl<Balance, BlockNumber, MaxMultiaddrLength> core::fmt::Debug
    for ProviderInfo<Balance, BlockNumber, MaxMultiaddrLength>
where
    Balance: Encode + Decode + MaxEncodedLen + core::fmt::Debug,
    BlockNumber: Encode + Decode + MaxEncodedLen + core::fmt::Debug,
    MaxMultiaddrLength: Get<u32>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProviderInfo")
            .field("multiaddr", &self.multiaddr)
            .field("public_key", &self.public_key)
            .field("stake", &self.stake)
            .field("committed_bytes", &self.committed_bytes)
            .field("settings", &self.settings)
            .field("stats", &self.stats)
            .field("deregister_at", &self.deregister_at)
            .finish()
    }
}

/// Provider settings controlling pricing and availability.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(
        bound(deserialize = "Balance: serde::Deserialize<'de> + Zero, \
                           BlockNumber: serde::Deserialize<'de> + Zero + Bounded"),
        rename_all = "camelCase",
        default
    )
)]
pub struct ProviderSettings<Balance, BlockNumber> {
    /// Minimum agreement duration provider will accept.
    pub min_duration: BlockNumber,
    /// Maximum agreement duration provider will accept.
    pub max_duration: BlockNumber,
    /// Price per byte per block for storage.
    pub price_per_byte: Balance,
    /// Whether accepting new primary agreements.
    pub accepting_primary: bool,
    /// Price per successful sync confirmation, or None if not accepting replicas.
    pub replica_sync_price: Option<Balance>,
    /// Whether accepting extensions on existing agreements.
    pub accepting_extensions: bool,
    /// Maximum storage capacity in bytes. 0 = unlimited (backward compatible).
    /// When set, provider cannot accept agreements that would exceed this capacity.
    pub max_capacity: u64,
}

impl<Balance: Zero, BlockNumber: Zero + Bounded> Default
    for ProviderSettings<Balance, BlockNumber>
{
    fn default() -> Self {
        Self {
            min_duration: Zero::zero(),
            max_duration: BlockNumber::max_value(),
            price_per_byte: Zero::zero(),
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0, // 0 = unlimited (backward compatible)
        }
    }
}

/// On-chain statistics for evaluating provider quality.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, Default)]
pub struct ProviderStats<BlockNumber> {
    /// Block when provider registered.
    pub registered_at: BlockNumber,
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

/// Why a provider was slashed via the challenge mechanism.
///
/// Emitted in `ChallengeSlashed` so observers can distinguish a provider that
/// went silent (Timeout) from one that submitted a demonstrably-false response
/// (InvalidProof, InvalidDeletionClaim, InvalidSupersededClaim).
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SlashReason {
    /// Provider failed to respond before the challenge deadline.
    Timeout,
    /// Provider submitted a `Proof` response whose chunk-Merkle or MMR proof
    /// did not verify.
    InvalidProof,
    /// Provider submitted a `Deleted` response with a signature or
    /// `new_start_seq` that does not stand up against on-chain state.
    InvalidDeletionClaim,
    /// Provider claimed `Superseded` but the bucket's canonical snapshot
    /// does not actually cover the challenged sequence.
    InvalidSupersededClaim,
}
