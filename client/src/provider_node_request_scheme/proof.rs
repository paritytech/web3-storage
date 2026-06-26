// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

/// Query for MMR proof.
#[derive(Debug, Clone, Deserialize)]
pub struct MmrProofQuery {
    pub bucket_id: BucketId,
    pub leaf_index: u64,
}

/// MMR leaf data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmrLeafData {
    pub data_root: String,
    pub data_size: u64,
    pub total_size: u64,
}

/// MMR proof response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmrProofResponse {
    pub leaf: MmrLeafData,
    pub proof: MmrProofData,
}

/// MMR proof data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmrProofData {
    pub peaks: Vec<String>,
    pub siblings: Vec<String>,
    pub path: Vec<bool>,
}

/// Query for chunk proof.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkProofQuery {
    pub data_root: String,
    pub chunk_index: u64,
}

/// Chunk proof response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkProofResponse {
    pub chunk_hash: String,
    /// Base64-encoded chunk data (included for challenge responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_data: Option<String>,
    pub proof: MerkleProofData,
}

/// Merkle proof data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProofData {
    pub siblings: Vec<String>,
    pub path: Vec<bool>,
}
