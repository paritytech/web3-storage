//! Provider-signed terms of a storage agreement.
//!
//! A provider quotes terms off-chain (e.g. over HTTP) and signs the SCALE
//! encoding of an `AgreementTerms` value. The owner then submits the signed
//! terms on-chain via `establish_agreement`, which verifies the signature,
//! checks the replay window (see [`crate::provider_replay_state`]), and
//! creates the bucket + agreement atomically.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::fmt::Debug;
use scale_info::TypeInfo;

/// Off-chain quote signed by the provider and redeemed on-chain by the owner.
///
/// Generic over the account/balance/block-number types so the same shape can
/// be reused by the pallet (with `BalanceOf<T>`/`BlockNumberFor<T>`), the
/// client SDK, and external tooling.
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
}
