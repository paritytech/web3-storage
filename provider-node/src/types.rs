// SPDX-License-Identifier: GPL-3.0-only

//! API types for the provider node.

use serde::{Deserialize, Serialize};
use storage_client::discovery::ProviderInfo;
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
    /// `CommitmentPayload` nonce for the provider's post-deletion signature.
    pub nonce: u64,
}

/// Response from delete operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
    pub nonce: u64,
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
    pub current_root: String,
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
    pub local_mmr_root: String,
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
