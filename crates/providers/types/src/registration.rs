// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

/// The node's view of its on-chain provider registration.
///
/// Decoded from the `StorageProvider::Providers` storage entry by the
/// chain-state coordinator; consumed by `/negotiate` validation and `/info`.
///
/// All durations and block numbers on this struct are counted in anchor
/// (relay-chain) blocks, 6s each - the same clock the pallet reads via its
/// `BlockNumberProvider` - not parachain blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Network address for connecting.
    pub multiaddr: String,
    /// Raw registered public key bytes. `/negotiate` refuses to sign while
    /// this doesn't match the local signing key.
    pub public_key: Vec<u8>,
    /// Total stake locked.
    pub stake: u128,
    /// Currently committed bytes.
    pub committed_bytes: u64,
    /// Maximum capacity (0 = unlimited).
    pub max_capacity: u64,
    /// Minimum agreement duration, in anchor (relay-chain) blocks.
    pub min_duration: u32,
    /// Maximum agreement duration, in anchor (relay-chain) blocks.
    pub max_duration: u32,
    /// Price per byte per anchor (relay-chain) block.
    pub price_per_byte: u128,
    /// Whether accepting primary agreements.
    pub accepting_primary: bool,
    /// Replica sync price (None if not accepting replicas).
    pub replica_sync_price: Option<u128>,
    /// Whether accepting extensions.
    pub accepting_extensions: bool,
    /// Total agreements ever.
    pub agreements_total: u32,
    /// Failed challenges count.
    pub challenges_failed: u32,
    /// Anchor (relay-chain) block at which deregistration becomes finalisable
    /// (`None` = not deregistering).
    pub deregister_at: Option<u32>,
}
