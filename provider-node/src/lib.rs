//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)
//! - Coordinating provider-initiated checkpoints

pub mod api;
pub mod challenge_responder;
pub mod checkpoint_coordinator;
pub mod disk_storage;
pub mod error;
pub mod mmr;
pub mod replica_sync;
pub mod replica_sync_coordinator;
pub mod storage;
pub mod types;

pub use api::create_router;
pub use challenge_responder::{
    ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle,
    ChallengeResponseResult, DetectedChallenge, ResponderCommand,
};
pub use checkpoint_coordinator::{
    CheckpointCoordinator, CheckpointCoordinatorConfig, CheckpointCoordinatorHandle,
    CheckpointDuty, CheckpointResult, CoordinatorCommand,
};
pub use disk_storage::DiskStorage;
pub use error::Error;
pub use replica_sync::ReplicaSync;
pub use replica_sync_coordinator::{
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, ReplicaSyncCoordinatorHandle,
    SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use storage::{Storage, StoredNode};
pub use types::*;

use sp_core::{crypto::Ss58Codec, sr25519, Pair, H256};
use std::sync::Arc;
use storage_primitives::{hash_children, BucketId};

/// Bucket information returned by the storage backend.
#[derive(Debug, Clone)]
pub struct BucketInfo {
    /// Current MMR root
    pub mmr_root: H256,
    /// Start sequence number
    pub start_seq: u64,
    /// Number of leaves in the MMR
    pub leaf_count: u64,
}

/// Build a Merkle proof for a leaf at the given index in a balanced (padded) tree.
///
/// Pads the leaf hashes to the next power of 2 with `H256::zero()` so that
/// the standard index-based verification in `verify_merkle_proof` works.
pub fn build_merkle_proof(leaf_hashes: &[H256], index: usize) -> storage_primitives::MerkleProof {
    if leaf_hashes.len() <= 1 {
        return storage_primitives::MerkleProof {
            siblings: vec![],
            path: vec![],
        };
    }

    // Pad to next power of 2 for a balanced tree
    let padded_len = leaf_hashes.len().next_power_of_two();
    let mut current_level = leaf_hashes.to_vec();
    current_level.resize(padded_len, H256::zero());

    let mut siblings = Vec::new();
    let mut path = Vec::new();
    let mut idx = index;

    while current_level.len() > 1 {
        let is_right = idx % 2 == 1;
        let sibling_idx = if is_right { idx - 1 } else { idx + 1 };
        siblings.push(current_level[sibling_idx]);
        path.push(is_right);

        // Build next level
        let mut next_level = Vec::new();
        for pair in current_level.chunks(2) {
            next_level.push(hash_children(pair[0], pair[1]));
        }

        idx /= 2;
        current_level = next_level;
    }

    storage_primitives::MerkleProof { siblings, path }
}

/// Trait for storage backends (in-memory, disk, etc.).
///
/// Both `Storage` (in-memory) and `DiskStorage` (persistent) implement this trait,
/// allowing the provider node to select the storage backend at startup.
/// The disk backend is currently backed by RocksDB but the implementation may change.
pub trait StorageBackend: Send + Sync {
    /// Initialize a bucket with the given quota.
    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error>;

    /// Get bucket information.
    fn get_bucket(&self, bucket_id: BucketId) -> Option<BucketInfo>;

    /// List all buckets.
    fn list_buckets(&self) -> Vec<BucketSummary>;

    /// Get per-bucket storage statistics.
    fn get_bucket_stats(&self) -> Vec<BucketStats>;

    /// Get total node count across all buckets.
    fn total_nodes(&self) -> u64;

    /// Get total bytes stored across all nodes.
    fn total_bytes(&self) -> u64;

    /// Store a node (chunk or internal node).
    fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> Result<(), Error>;

    /// Get a node by hash.
    fn get_node(&self, hash: &H256) -> Option<StoredNode>;

    /// Check which hashes exist in storage.
    fn check_exists(&self, bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>);

    /// Commit data roots to the bucket's MMR.
    fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<(H256, u64, Vec<u64>), Error>;

    /// Collect leaf chunk hashes under a data root (DFS, in order).
    fn collect_chunk_hashes(&self, root: H256) -> Vec<H256> {
        let mut hashes = Vec::new();
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if hash == H256::zero() {
                continue;
            }
            if let Some(node) = self.get_node(&hash) {
                if let Some(ref children) = node.children {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                } else {
                    hashes.push(hash);
                }
            }
        }

        hashes
    }

    /// Get chunk data and Merkle proof at the given index from a data root.
    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, storage_primitives::MerkleProof), Error> {
        let chunk_hashes = self.collect_chunk_hashes(data_root);

        if chunk_index as usize >= chunk_hashes.len() {
            return Err(Error::NodeNotFound(format!("chunk_{chunk_index}")));
        }

        let chunk_hash = chunk_hashes[chunk_index as usize];
        let chunk_data = self
            .get_node(&chunk_hash)
            .ok_or_else(|| Error::NodeNotFound(format!("chunk_data_{chunk_index}")))?
            .data;

        let proof = build_merkle_proof(&chunk_hashes, chunk_index as usize);

        Ok((chunk_data, proof))
    }

    /// Delete data before a sequence number.
    fn delete_before(
        &self,
        bucket_id: BucketId,
        new_start_seq: u64,
    ) -> Result<(H256, u64, u64), Error>;

    /// Get MMR proof for a leaf.
    fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error>;

    /// Get MMR peaks.
    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error>;
}

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<dyn StorageBackend>,
    /// Provider account ID (SS58 encoded)
    pub provider_id: String,
    /// Signing keypair (optional, for dev/testing)
    pub keypair: Option<sr25519::Pair>,
}

impl ProviderState {
    pub fn new(storage: Arc<dyn StorageBackend>, provider_id: String) -> Self {
        Self {
            storage,
            provider_id,
            keypair: None,
        }
    }

    /// Create with a seed phrase or derivation path (e.g., "//Alice", "//Bob").
    pub fn with_seed(storage: Arc<dyn StorageBackend>, seed: &str) -> Result<Self, String> {
        let keypair = sr25519::Pair::from_string(seed, None)
            .map_err(|e| format!("Failed to create keypair: {e:?}"))?;

        let provider_id = keypair.public().to_ss58check();

        Ok(Self {
            storage,
            provider_id,
            keypair: Some(keypair),
        })
    }

    /// Sign a message and return the signature as hex.
    pub fn sign(&self, message: &[u8]) -> String {
        match &self.keypair {
            Some(keypair) => {
                let signature = keypair.sign(message);
                format!("0x{}", hex::encode(signature.0))
            }
            None => {
                // Return placeholder if no keypair configured
                format!("0x{}", hex::encode([0u8; 64]))
            }
        }
    }
}
