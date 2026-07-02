// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Query for getting commitment.
#[derive(Debug, Clone, Deserialize)]
pub struct CommitmentQuery {
    pub bucket_id: BucketId,
    /// `CommitmentPayload` nonce. See [`CommitRequest::nonce`].
    pub nonce: u64,
}

/// Response with current commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
    pub nonce: u64,
}

/// Response with checkpoint-compatible signature (signs with real leaf_count).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSignatureResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
    pub nonce: u64,
}

/// Response from triggering a checkpoint via the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCheckpointResponse {
    pub bucket_id: BucketId,
    pub triggered: bool,
    pub message: String,
}
