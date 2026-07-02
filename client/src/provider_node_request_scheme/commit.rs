// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Request to commit data roots to MMR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRequest {
    pub bucket_id: BucketId,
    /// Data roots to add to the MMR
    pub data_roots: Vec<String>,
    /// `CommitmentPayload` nonce — block at which the caller expects to
    /// submit the resulting signature on-chain. The provider signs over
    /// this value so the pallet's recency check passes.
    pub nonce: u64,
}

/// Response from commit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    /// Number of leaves in the MMR after the commit.
    pub leaf_count: u64,
    /// Leaf indices assigned to each data root
    pub leaf_indices: Vec<u64>,
    /// Provider signature over the commitment
    pub provider_signature: String,
    /// Echo of the nonce the provider signed over (the same value the caller
    /// passed in). Returned for symmetry so downstream code doesn't have to
    /// thread it through manually.
    pub nonce: u64,
}
