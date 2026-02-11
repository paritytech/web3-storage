//! API types for the provider node.

use serde::{Deserialize, Serialize};
use sp_core::H256;
use storage_primitives::BucketId;

// ─────────────────────────────────────────────────────────────────────────────
// Node Upload/Download Types
// ─────────────────────────────────────────────────────────────────────────────

/// Request to upload a node (chunk or internal node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadNodeRequest {
    /// Bucket this node belongs to
    pub bucket_id: BucketId,
    /// Expected hash of the node data
    pub hash: String,
    /// Base64-encoded node data
    pub data: String,
    /// Child hashes for internal nodes, null for leaf chunks
    pub children: Option<Vec<String>>,
}

/// Response from uploading a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadNodeResponse {
    pub stored: bool,
}

/// Response from downloading a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadNodeResponse {
    pub hash: String,
    pub data: String,
    pub children: Option<Vec<String>>,
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
    pub exists: Vec<String>,
    pub missing: Vec<String>,
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
    pub mmr_root: String,
    pub start_seq: u64,
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
    pub data_root: String,
    pub offset: u64,
    pub length: u64,
}

/// A chunk with its proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkWithProof {
    pub hash: String,
    pub data: String,
    pub proof: Vec<String>,
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
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}

/// Response with checkpoint-compatible signature (signs with real leaf_count).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSignatureResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
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
    pub proof: MerkleProofData,
}

/// Merkle proof data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProofData {
    pub siblings: Vec<String>,
    pub path: Vec<bool>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Delete Types
// ─────────────────────────────────────────────────────────────────────────────

/// Request to delete data (admin only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub bucket_id: BucketId,
    pub new_start_seq: u64,
    /// Admin signature authorizing deletion
    pub admin_signature: String,
}

/// Response from delete operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Bucket Types
// ─────────────────────────────────────────────────────────────────────────────

/// Bucket summary info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSummary {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
}

/// Response with bucket list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBucketsResponse {
    pub buckets: Vec<BucketSummary>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Health/Info Types
// ─────────────────────────────────────────────────────────────────────────────

/// Provider info response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoResponse {
    pub status: String,
    pub version: String,
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
    pub mmr_root: String,
    pub peaks: Vec<String>,
}

/// Query for MMR subtree.
#[derive(Debug, Clone, Deserialize)]
pub struct MmrSubtreeQuery {
    pub bucket_id: BucketId,
    pub peak_index: u32,
    pub depth: u32,
}

/// MMR node info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmrNode {
    pub position: u64,
    pub hash: String,
    pub children: Option<Vec<u64>>,
}

/// Response with MMR subtree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmrSubtreeResponse {
    pub nodes: Vec<MmrNode>,
}

/// Request to fetch multiple nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchNodesRequest {
    pub bucket_id: BucketId,
    pub hashes: Vec<String>,
}

/// Fetched node data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedNode {
    pub hash: String,
    pub data: String,
    pub children: Option<Vec<String>>,
}

/// Response with fetched nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchNodesResponse {
    pub nodes: Vec<FetchedNode>,
}
