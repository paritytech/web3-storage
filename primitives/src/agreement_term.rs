//! Provider-signed terms of a storage agreement.
//!
//! A provider quotes terms off-chain (e.g. over HTTP) and signs the SCALE
//! encoding of an `AgreementTerms` value.
//!
//! [`AgreementTerms`] shape covers both flavours: `replica` is
//! `None` for primary agreements and `Some(_)` for replica agreements,
//! carrying the per-sync funding parameters.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use scale_info::TypeInfo;

/// Off-chain quote signed by the provider and redeemed on-chain by the owner.
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AgreementTerms<AccountId, Balance, BlockNumber> {
    /// Owner that will be bound by these terms (must match the extrinsic
    /// origin at redemption).
    pub owner: AccountId,
    /// Storage quota committed by the provider, in bytes.
    pub max_bytes: u64,
    /// Agreement duration in blocks from activation.
    pub duration: BlockNumber,
    /// Price per byte per block locked at quote time.
    pub price_per_byte: Balance,
    /// Block number after which the quote is no longer redeemable.
    pub valid_until: BlockNumber,
    /// Provider-chosen replay-protection nonce; uniqueness is enforced
    /// through the provider's sliding replay window.
    pub nonce: u64,
    /// Bucket the quote is bound to.
    /// - `None` for primary terms
    /// - `Some(id)` for replica terms — must match the bucket targeted by
    ///   the extrinsic.
    pub bucket_id: Option<crate::BucketId>,
    /// Replica-specific parameters.
    /// - `None` means these are primary terms;
    /// - `Some(_)` means the provider has quoted a replica agreement and the extra per-sync funding is included.
    pub replica_params: Option<ReplicaTerms<Balance, BlockNumber>>,
}

/// Replica terms
#[derive(
    Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplicaTerms<Balance, BlockNumber> {
    /// Balance reserved by the owner to fund per-sync confirmations. The
    /// pallet draws down `sync_price` from this on each accepted sync.
    pub sync_balance: Balance,
    /// Minimum blocks between sync confirmations the provider commits to.
    pub min_sync_interval: BlockNumber,
}
