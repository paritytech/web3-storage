// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Request to commit data roots to MMR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRequest {
    pub bucket_id: BucketId,
    /// Data roots to add to the MMR
    pub data_roots: Vec<String>,
}

/// Response from commit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    /// Leaf indices assigned to each data root
    pub leaf_indices: Vec<u64>,
    /// Provider signature over the commitment
    pub provider_signature: String,
}
