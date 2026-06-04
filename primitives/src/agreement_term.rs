//! Provider-signed terms of a storage agreement.
//!
//! A provider quotes terms off-chain (e.g. over HTTP) and signs
//! `blake2_256(TERM_CONTEXT | SCALE(terms))`, where the context string
//! domain-separates primary quotes from replica quotes.
//!
//! [`AgreementTerms`] shape covers both flavours: `replica` is
//! `None` for primary agreements and `Some(_)` for replica agreements,
//! carrying the per-sync funding parameters.

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use scale_info::TypeInfo;

/// Domain-separation context prefixed to the signing payload of primary terms.
pub const PRIMARY_TERM_CONTEXT: &[u8] = b"primary-term-v1:";

/// Domain-separation context prefixed to the signing payload of replica terms.
pub const REPLICA_TERM_CONTEXT: &[u8] = b"replica-term-v1:";

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

impl<AccountId: Encode, Balance: Encode, BlockNumber: Encode>
    AgreementTerms<AccountId, Balance, BlockNumber>
{
    /// Domain-separation context matching this quote's flavour:
    /// [`REPLICA_TERM_CONTEXT`] when `replica_params` is `Some(_)`,
    /// [`PRIMARY_TERM_CONTEXT`] otherwise.
    pub fn signing_context(&self) -> &'static [u8] {
        if self.replica_params.is_some() {
            REPLICA_TERM_CONTEXT
        } else {
            PRIMARY_TERM_CONTEXT
        }
    }

    /// Bytes the provider hashes (blake2-256) and signs:
    /// `signing_context() | SCALE(self)`.
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut payload = self.signing_context().to_vec();
        self.encode_to(&mut payload);
        payload
    }
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
