// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Request to delete data (admin only).
///
/// Authorization is carried in the `Authorization` header (`Web3Storage …`,
/// verified against the bucket's Admin members), not in the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub bucket_id: BucketId,
    pub new_start_seq: u64,
}

/// Response from delete operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}
