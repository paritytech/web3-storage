// SPDX-License-Identifier: Apache-2.0

//! Runtime API for querying storage provider pallet state.
//!
//! This provides read-only queries for applications to discover providers,
//! bucket state, agreements, and challenges without submitting transactions.

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_core::H256;
use storage_primitives::{BucketId, BucketSnapshot, ProviderRole, Role};

/// Provider information returned by runtime API.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ProviderInfoResponse {
    pub multiaddr: Vec<u8>,
    pub public_key: Vec<u8>,
    pub stake: u128,
    pub committed_bytes: u64,
    pub min_duration: u32,
    pub max_duration: u32,
    pub price_per_byte: u128,
    pub accepting_primary: bool,
    pub replica_sync_price: Option<u128>,
    pub accepting_extensions: bool,
    pub registered_at: u32,
    pub agreements_total: u32,
    pub agreements_extended: u32,
    pub agreements_not_extended: u32,
    pub agreements_burned: u32,
    pub challenges_received: u32,
    pub challenges_failed: u32,
    /// Maximum storage capacity in bytes (0 = unlimited).
    pub max_capacity: u64,
    /// Available capacity in bytes (None if unlimited).
    pub available_capacity: Option<u64>,
    /// Anchor block at which deregistration becomes finalisable
    /// (`None` = not deregistering).
    pub deregister_at: Option<u32>,
    /// Reputation 0-100, from [`reputation_score`]. Carried here so clients
    /// never re-implement the formula.
    pub reputation: u8,
}

/// Storage requirements for provider matching.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageRequirements {
    /// Bytes needed for storage.
    pub bytes_needed: u64,
    /// Minimum agreement duration in blocks.
    pub min_duration: u32,
    /// Maximum acceptable price per byte.
    pub max_price_per_byte: u128,
    /// If true, only match providers accepting primary agreements.
    pub primary_only: bool,
}

/// Reason for partial match when provider doesn't fully meet requirements.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum PartialMatchReason {
    /// Provider's price exceeds max_price_per_byte.
    PriceTooHigh,
    /// Provider doesn't have enough available capacity.
    InsufficientCapacity,
    /// Provider's duration constraints don't match.
    DurationMismatch,
    /// Provider is not accepting agreements.
    NotAccepting,
}

/// Provider matching result.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MatchedProvider {
    /// Provider account ID (encoded).
    pub account: Vec<u8>,
    /// Provider information.
    pub info: ProviderInfoResponse,
    /// Match score (0-100, 100 = perfect match).
    pub match_score: u8,
    /// Available capacity in bytes (None if unlimited).
    pub available_capacity: Option<u64>,
    /// If not a perfect match, why.
    pub partial_reason: Option<PartialMatchReason>,
}

/// Bucket member information.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct BucketMemberResponse {
    pub account: Vec<u8>, // Encoded AccountId
    pub role: Role,
}

/// Bucket information returned by runtime API.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct BucketResponse {
    pub bucket_id: BucketId,
    pub members: Vec<BucketMemberResponse>,
    pub frozen_start_seq: Option<u64>,
    pub min_providers: u32,
    pub primary_providers: Vec<Vec<u8>>, // Vec of encoded AccountIds
    pub snapshot: Option<BucketSnapshot<u32>>,
    pub total_snapshots: u32,
}

/// Storage agreement information.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct AgreementResponse {
    pub bucket_id: BucketId,
    pub owner: Vec<u8>,    // Encoded AccountId
    pub provider: Vec<u8>, // Encoded AccountId
    pub max_bytes: u64,
    pub payment_locked: u128,
    pub price_per_byte: u128,
    pub expires_at: u32,
    pub extensions_blocked: bool,
    pub role: ProviderRole<u128, u32>,
    pub started_at: u32,
}

/// Upper bound on the number of candidates `challenge_candidates` returns, so
/// a caller-supplied `limit` cannot ask for an unbounded response.
pub const MAX_CHALLENGE_CANDIDATES: u32 = 256;

/// A provider worth challenging: it holds at least one storage agreement and
/// its reputation is below the caller's threshold.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeCandidate {
    /// One of the buckets this provider stores for — the challenge target.
    pub bucket_id: BucketId,
    pub provider: Vec<u8>, // Encoded AccountId
    pub stake: u128,
    pub challenges_received: u32,
    pub challenges_failed: u32,
    /// Reputation 0–100, from [`reputation_score`].
    pub reputation: u8,
}

/// A provider's 0–100 reputation from its on-chain challenge record.
///
/// Providers with no recorded challenges score 100 — benefit of the doubt, so
/// a newly registered provider is not immediately challenge-worthy.
pub fn reputation_score(challenges_received: u32, challenges_failed: u32) -> u8 {
    if challenges_received == 0 {
        return 100;
    }
    let defended = challenges_received.saturating_sub(challenges_failed);
    ((defended as u64 * 100) / challenges_received as u64).min(100) as u8
}

/// Challenge information.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeResponse {
    pub bucket_id: BucketId,
    pub provider: Vec<u8>,   // Encoded AccountId
    pub challenger: Vec<u8>, // Encoded AccountId
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_index: u64,
    pub chunk_index: u64,
    pub deadline: u32,
    /// Stable per-deadline index, forming `ChallengeId { deadline, index }`.
    pub index: u16,
    pub deposit: u128,
}

sp_api::decl_runtime_apis! {
    /// Runtime API for the storage provider pallet.
    ///
    /// v2 reshaped `ProviderInfoResponse` (`deregister_at`, `reputation`) and added
    /// `challenge_candidates`. Declared explicitly so callers can probe the version
    /// instead of decoding a v1 shape that no longer exists.
    #[api_version(2)]
    pub trait StorageProviderApi<AccountId, BlockNumber, Balance>
    where
        AccountId: Encode + Decode,
        BlockNumber: Encode + Decode,
        Balance: Encode + Decode,
    {
        /// Get provider information.
        fn provider_info(provider: AccountId) -> Option<ProviderInfoResponse>;

        /// Get all registered providers (paginated).
        fn providers(offset: u32, limit: u32) -> Vec<(AccountId, ProviderInfoResponse)>;

        /// Get bucket information.
        fn bucket_info(bucket_id: BucketId) -> Option<BucketResponse>;

        /// Get all bucket IDs (paginated).
        fn bucket_ids(offset: u32, limit: u32) -> Vec<BucketId>;

        /// Get providers for a specific bucket.
        fn bucket_providers(bucket_id: BucketId) -> Vec<AccountId>;

        /// Get agreement information.
        fn agreement_info(bucket_id: BucketId, provider: AccountId) -> Option<AgreementResponse>;

        /// Get all agreements for a bucket.
        fn bucket_agreements(bucket_id: BucketId) -> Vec<AgreementResponse>;

        /// Get all agreements for a provider.
        fn provider_agreements(provider: AccountId) -> Vec<AgreementResponse>;

        /// Get challenges expiring at a specific deadline. `block` is an anchor
        /// block (see `current_anchor_block`), not a parachain height.
        fn challenges_at(block: BlockNumber) -> Vec<ChallengeResponse>;

        /// Get all challenges for a specific bucket.
        fn bucket_challenges(bucket_id: BucketId) -> Vec<ChallengeResponse>;

        /// Get all challenges targeting a specific provider.
        fn provider_challenges(provider: AccountId) -> Vec<ChallengeResponse>;

        /// Get all challenges created by a specific challenger.
        fn challenger_challenges(challenger: AccountId) -> Vec<ChallengeResponse>;

        /// Check if a provider has sufficient stake for additional bytes.
        fn can_accept_bytes(provider: AccountId, additional_bytes: u64) -> bool;

        /// Find providers matching the given storage requirements.
        /// Returns up to `limit` providers, sorted by match score (best first).
        fn find_matching_providers(requirements: StorageRequirements, limit: u32) -> Vec<MatchedProvider>;

        /// Get providers with sufficient capacity for the given bytes (paginated).
        fn providers_with_capacity(bytes_needed: u64, offset: u32, limit: u32) -> Vec<(AccountId, ProviderInfoResponse)>;

        /// Returns providers worth challenging, worst reputation first.
        ///
        /// A provider qualifies if it holds at least one storage agreement and
        /// its reputation is strictly below `max_reputation`. Each provider
        /// appears once, paired with one of its buckets, so a caller challenges
        /// it at most once per round.
        ///
        /// Reputation runs from 0 to 100 (see [`reputation_score`]).
        /// `max_reputation` saturates outside that range instead of erroring:
        /// `0` matches nothing, and any value above 100 disables the filter.
        ///
        /// `limit` is clamped to [`MAX_CHALLENGE_CANDIDATES`]; it bounds the
        /// response, not the underlying scan.
        fn challenge_candidates(max_reputation: u8, limit: u32) -> Vec<ChallengeCandidate>;

        /// The anchor block every on-chain duration (timeouts, expiries,
        /// `valid_until`, nonce age) is measured against. Off-chain actors read
        /// this instead of a specific storage item so they need not know whether
        /// the anchor is a relay, parachain, or other block number.
        fn current_anchor_block() -> BlockNumber;

        /// Milliseconds per anchor block. Pairs with `current_anchor_block` so
        /// off-chain consumers can humanize anchor-denominated durations
        /// without knowing which clock the pallet measures them on.
        fn anchor_block_time_millis() -> u64;
    }
}
