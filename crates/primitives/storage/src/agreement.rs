// SPDX-License-Identifier: Apache-2.0

//! Storage agreements between buckets and providers.

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use crate::ProviderRole;

/// Storage agreement between bucket and provider.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
pub struct StorageAgreement<AccountId, Balance, BlockNumber> {
    /// Who owns this agreement (can top up, transfer ownership).
    pub owner: AccountId,
    /// Maximum bytes (quota).
    pub max_bytes: u64,
    /// Payment locked for storage.
    pub payment_locked: Balance,
    /// Price per byte locked at creation/last extension.
    pub price_per_byte: Balance,
    /// Agreement expiration.
    pub expires_at: BlockNumber,
    /// Whether provider has blocked extensions for this agreement.
    pub extensions_blocked: bool,
    /// Provider role for this bucket.
    pub role: ProviderRole<Balance, BlockNumber>,
    /// Block when agreement became active.
    pub started_at: BlockNumber,
}
