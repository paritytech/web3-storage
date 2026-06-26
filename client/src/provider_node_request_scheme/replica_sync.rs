// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};
use storage_primitives::BucketId;

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
