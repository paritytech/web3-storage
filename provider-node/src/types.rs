// SPDX-License-Identifier: GPL-3.0-only

//! API types for the provider node.

use crate::error::Error;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use provider_storage::ChunkTreeNode;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use storage_primitives::BucketId;

pub use provider_types::ProviderInfo;

// ─────────────────────────────────────────────────────────────────────────────
// Node Upload/Download Types
// ─────────────────────────────────────────────────────────────────────────────

/// Decode a `0x`-prefixed or bare hex hash. Returns `InvalidHash` if the hex is
/// invalid or the length is not 32 bytes.
pub(crate) fn decode_hash(hash: &str) -> Result<H256, Error> {
    hash.parse::<H256>().map_err(|e| Error::InvalidHash {
        expected: hash.to_string(),
        actual: e.to_string(),
    })
}

/// Request to upload a node: a chunk (`data`) or an internal node
/// (`children`). A request with both or neither is rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadNodeRequest {
    /// Bucket this node belongs to
    pub bucket_id: BucketId,
    /// Expected hash of the node: the chunk hash for a chunk, the hash of
    /// the two children for an internal node
    pub hash: String,
    /// Base64-encoded chunk bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Left and right child hashes of an internal node
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<String>>,
}

/// Response from uploading a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadNodeResponse {
    pub stored: bool,
}

/// Response from downloading a node: a chunk has `data`, an internal node has
/// `children`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadNodeResponse {
    pub hash: H256,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<H256>>,
}

impl DownloadNodeResponse {
    /// The wire form of `node`, which is stored under `hash`.
    pub fn new(hash: H256, node: &ChunkTreeNode) -> Self {
        let (data, children) = match node {
            ChunkTreeNode::Chunk(data) => (Some(BASE64.encode(data)), None),
            ChunkTreeNode::Internal { left, right } => (None, Some(vec![*left, *right])),
        };
        Self {
            hash,
            data,
            children,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Existence Check Types
// ─────────────────────────────────────────────────────────────────────────────

/// Request to check existence of nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsRequest {
    pub bucket_id: BucketId,
    pub hashes: Vec<String>,
}

/// Response with existing and missing nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsResponse {
    pub exists: Vec<H256>,
    pub missing: Vec<H256>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Commit Types
// ─────────────────────────────────────────────────────────────────────────────

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
    pub mmr_root: H256,
    pub start_seq: u64,
    /// Leaf count of the MMR after this commit (signed into the commitment).
    pub leaf_count: u64,
    /// Leaf indices assigned to each data root
    pub leaf_indices: Vec<u64>,
    /// Provider signature over the commitment
    pub provider_signature: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Read Types
// ─────────────────────────────────────────────────────────────────────────────

/// Query parameters for reading chunks.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadQuery {
    pub bucket_id: BucketId,
    pub data_root: String,
    pub offset: u64,
    pub length: u64,
}

/// A chunk with its proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkWithProof {
    pub hash: H256,
    pub data: String,
    pub proof: Vec<H256>,
}

/// Response with chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResponse {
    pub chunks: Vec<ChunkWithProof>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Commitment Types
// ─────────────────────────────────────────────────────────────────────────────

/// Query for getting commitment.
#[derive(Debug, Clone, Deserialize)]
pub struct CommitmentQuery {
    pub bucket_id: BucketId,
}

/// Response with current commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentResponse {
    pub bucket_id: BucketId,
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}

/// Response with checkpoint-compatible signature (signs with real leaf_count).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSignatureResponse {
    pub bucket_id: BucketId,
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Proof Types
// ─────────────────────────────────────────────────────────────────────────────

/// Query for MMR proof.
#[derive(Debug, Clone, Deserialize)]
pub struct MmrProofQuery {
    pub bucket_id: BucketId,
    pub leaf_index: u64,
}

/// Query for chunk proof.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkProofQuery {
    pub bucket_id: BucketId,
    pub data_root: String,
    pub chunk_index: u64,
}

/// Chunk proof response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkProofResponse {
    pub chunk_hash: H256,
    /// Base64-encoded chunk data (included for challenge responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_data: Option<String>,
    /// Sibling hashes from the chunk up to `data_root`.
    pub proof: Vec<H256>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Delete Types
// ─────────────────────────────────────────────────────────────────────────────

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
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Health/Info Types
// ─────────────────────────────────────────────────────────────────────────────

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

/// Provider statistics: node-wide totals only. `GET /stats` is
/// unauthenticated, so a per-bucket breakdown would list the bucket ids that
/// the per-bucket read gates hide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub provider_id: String,
    pub total_buckets: usize,
    pub total_nodes: u64,
    pub total_bytes: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Replica Sync Types
// ─────────────────────────────────────────────────────────────────────────────

/// Query for MMR peaks.
#[derive(Debug, Clone, Deserialize)]
pub struct MmrPeaksQuery {
    pub bucket_id: BucketId,
}

/// Response with MMR peaks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmrPeaksResponse {
    pub bucket_id: BucketId,
    pub mmr_root: H256,
    pub peaks: Vec<H256>,
}

/// Request to fetch multiple nodes of a bucket. Like `GET /node`, the hash is
/// the read capability (see the L0 read-access rules in the design doc's
/// "Bucket Visibility & Access").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchNodesRequest {
    pub bucket_id: BucketId,
    pub hashes: Vec<String>,
}

/// Response with fetched nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchNodesResponse {
    pub nodes: Vec<DownloadNodeResponse>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Replica Sync Coordinator Types
// ─────────────────────────────────────────────────────────────────────────────

/// Query for historical roots.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoricalRootsQuery {
    pub bucket_id: BucketId,
}

/// Response with current and historical roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalRootsResponse {
    pub bucket_id: BucketId,
    /// Current MMR root (position 0).
    pub current_root: H256,
    /// Historical roots (positions 1-6).
    pub historical_roots: [String; 6],
    /// Block number of the snapshot.
    pub snapshot_block: u64,
}

/// Query for bucket sync status.
#[derive(Debug, Clone, Deserialize)]
pub struct BucketSyncStatusQuery {
    pub bucket_id: BucketId,
}

/// Response with bucket sync status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSyncStatusResponse {
    pub bucket_id: BucketId,
    /// Local MMR root.
    pub local_mmr_root: H256,
    /// Local leaf count.
    pub local_leaf_count: u64,
    /// Block number of last sync (if any).
    pub last_sync_block: Option<u64>,
    /// Whether sync is in progress.
    pub syncing: bool,
}

/// Request to force sync a bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceSyncRequest {
    pub bucket_id: BucketId,
}

/// Response from force sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceSyncResponse {
    pub bucket_id: BucketId,
    pub queued: bool,
    pub message: String,
}

/// Response with overall replica sync coordinator status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicaSyncCoordinatorStatusResponse {
    /// Whether coordinator is running.
    pub running: bool,
    /// Whether coordinator is paused.
    pub paused: bool,
    /// Number of active sync operations.
    pub active_syncs: usize,
    /// Buckets being tracked as replica.
    pub tracked_buckets: Vec<BucketId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_hash_accepts_32_bytes_with_or_without_prefix() {
        let hex = "ab".repeat(32);
        assert_eq!(decode_hash(&hex).unwrap(), H256::repeat_byte(0xab));
        assert_eq!(
            decode_hash(&format!("0x{hex}")).unwrap(),
            H256::repeat_byte(0xab)
        );
        for bad in ["ab".repeat(31), "ab".repeat(33), "zz".repeat(32)] {
            assert!(matches!(decode_hash(&bad), Err(Error::InvalidHash { .. })));
        }
    }
}
