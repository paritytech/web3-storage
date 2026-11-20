// SPDX-License-Identifier: Apache-2.0

use crate::ProviderInfo;
use parking_lot::RwLock;
use std::sync::atomic::AtomicU32;

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// The coordinator, the HTTP handlers and the provider signer each have their
/// own `Arc` to it.
#[derive(Default)]
pub struct ChainState {
    /// The pallet's anchor block — the clock all on-chain durations (timeouts,
    /// `valid_until`) are measured against — read via the
    /// `StorageProviderApi::current_anchor_block` runtime API at the latest
    /// finalized block. Whether that anchor is a relay, parachain, or other
    /// block number is the pallet's concern, not the provider's. `0` means not
    /// yet known.
    pub current_anchor_block: AtomicU32,
    /// Pallet constants fetched once per connection. `None` until the first
    /// successful fetch; `/negotiate` returns 503 until this is `Some`.
    pub constants: RwLock<Option<PalletConstants>>,
    /// Provider's on-chain registration info. `None` until registered on chain;
    /// re-fetched (full) on every relevant provider event so `committed_bytes`,
    /// `stake`, and all settings stay current.
    pub provider_info: RwLock<Option<ProviderInfo>>,
}

/// Pallet constants that only change across runtime upgrades.
pub struct PalletConstants {
    /// Chain-enforced validity window (in blocks) for provider-signed terms.
    pub request_timeout: u32,
}
