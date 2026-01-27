//! Runtime API for querying storage provider pallet state.
//!
//! This provides read-only queries for applications to discover providers,
//! bucket state, agreements, and challenges without submitting transactions.

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
    pub owner: Vec<u8>, // Encoded AccountId
    pub provider: Vec<u8>, // Encoded AccountId
    pub max_bytes: u64,
    pub payment_locked: u128,
    pub price_per_byte: u128,
    pub expires_at: u32,
    pub extensions_blocked: bool,
    pub role: ProviderRole<u128, u32>,
    pub started_at: u32,
}

/// Challenge information.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeResponse {
    pub bucket_id: BucketId,
    pub provider: Vec<u8>, // Encoded AccountId
    pub challenger: Vec<u8>, // Encoded AccountId
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_index: u64,
    pub chunk_index: u64,
    pub deadline: u32,
    pub deposit: u128,
}

sp_api::decl_runtime_apis! {
    /// Runtime API for the storage provider pallet.
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

        /// Get challenges expiring at a specific block.
        fn challenges_at(block: BlockNumber) -> Vec<ChallengeResponse>;

        /// Check if a provider has sufficient stake for additional bytes.
        fn can_accept_bytes(provider: AccountId, additional_bytes: u64) -> bool;
    }
}
