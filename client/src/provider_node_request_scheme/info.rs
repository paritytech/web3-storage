// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::ProviderInfo;

/// Provider info response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoResponse {
    pub provider_id: String,
    pub provider_registration_info: Option<ProviderInfo>,
    /// Readiness of the signing-bound endpoints (e.g. `/negotiate`). A
    /// provider can be registered and accepting agreements yet still reject
    /// `/negotiate` if these are not all `true`.
    pub readiness: ProviderReadiness,
}

/// Readiness flags for signing-bound endpoints, surfaced via `/info` so the
/// reason `/negotiate` is unavailable can be diagnosed without reading logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderReadiness {
    /// A signing keypair is configured (node started with `--keyfile`).
    pub signing_configured: bool,
    /// The nonce counter is bootstrapped from on-chain replay state.
    pub nonce_counter_ready: bool,
    /// On-chain provider registration info has been loaded.
    pub provider_info_loaded: bool,
    /// The provider has announced deregistration; `/negotiate` is disabled and
    /// returns 503 even when every other flag is `true`.
    pub deregistering: bool,
}

/// Health check response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Provider statistics response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub provider_id: String,
    pub total_buckets: usize,
    pub total_nodes: u64,
    pub total_bytes: u64,
    pub buckets: Vec<BucketStats>,
}

/// Per-bucket statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketStats {
    pub bucket_id: BucketId,
    pub leaf_count: u64,
    pub node_count: u64,
    pub bytes_stored: u64,
}
