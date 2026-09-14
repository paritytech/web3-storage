// SPDX-License-Identifier: Apache-2.0

use crate::runtime_type::{Balance, BlockNumber};
use serde::{Deserialize, Serialize};

/// The node's view of its on-chain provider registration.
///
/// Decoded from the `StorageProvider::Providers` storage entry by the
/// chain-state coordinator; consumed by `/negotiate` validation and `/info`.
///
/// All durations and block numbers on these structs are counted in anchor
/// (relay-chain) blocks, 6s each - the same clock the pallet reads via its
/// `BlockNumberProvider` - not parachain blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Network address for connecting to this provider.
    pub multiaddr: String,
    /// Raw registered public key bytes. `/negotiate` refuses to sign while
    /// this doesn't match the local signing key.
    pub public_key: Vec<u8>,
    /// Total stake locked by this provider.
    pub stake: Balance,
    /// Total contracted bytes (sum of `max_bytes` across all agreements).
    pub committed_bytes: u64,
    /// Provider settings.
    pub settings: ProviderSettings,
    /// Provider statistics.
    pub stats: ProviderStats,
    /// Anchor block at which a previously-announced deregistration becomes
    /// finalisable. `None` means no announcement is in progress.
    pub deregister_at: Option<BlockNumber>,
}

/// Provider settings controlling pricing and availability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Minimum agreement duration, in anchor blocks.
    pub min_duration: BlockNumber,
    /// Maximum agreement duration, in anchor blocks.
    pub max_duration: BlockNumber,
    /// Price per byte per anchor block.
    pub price_per_byte: Balance,
    /// Whether accepting new primary agreements.
    pub accepting_primary: bool,
    /// Price per successful sync confirmation, or `None` if not accepting
    /// replicas.
    pub replica_sync_price: Option<Balance>,
    /// Whether accepting extensions on existing agreements.
    pub accepting_extensions: bool,
    /// Maximum storage capacity in bytes. `0` = unlimited.
    pub max_capacity: u64,
}

/// On-chain statistics for evaluating provider quality.
///
/// `Default` mirrors the pallet's `DefaultNoBound`: an all-zero counter set,
/// as a freshly registered provider has.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderStats {
    /// Anchor block at which the provider registered.
    pub registered_at: BlockNumber,
    /// Total agreements ever created with this provider.
    pub agreements_total: u32,
    /// Agreements where the client chose to extend.
    pub agreements_extended: u32,
    /// Agreements that expired without extension.
    pub agreements_not_extended: u32,
    /// Agreements where the client burned payment.
    pub agreements_burned: u32,
    /// Total bytes ever committed across all agreements.
    pub total_bytes_committed: u64,
    /// Challenges from authorized challengers that the provider responded to.
    pub challenges_received_authorized: u32,
    /// Same, for general-public challengers.
    pub challenges_received_public: u32,
    /// Challenges where the provider was slashed.
    pub challenges_failed: u32,
}
