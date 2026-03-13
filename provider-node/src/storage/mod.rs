//! Storage backends for the provider node.
//!
//! This module contains the `StorageBackend` trait and its implementations:
//! - `Storage`: in-memory backend for testing and development
//! - `DiskStorage`: persistent RocksDB backend for production

pub mod disk;
pub mod in_memory;

pub use disk::DiskStorage;
pub use in_memory::Storage;

use crate::error::Error;
use crate::types::*;
use sp_core::H256;
use storage_primitives::{hash_children, BucketId};

/// A stored node (chunk or internal node).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredNode {
    /// The raw data
    pub data: Vec<u8>,
    /// Child hashes for internal nodes
    pub children: Option<Vec<H256>>,
}

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

    /// Collect actual chunk data under a data root (DFS, leaf data in order).
    fn collect_chunks(&self, root: H256) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
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
                    chunks.push(node.data.clone());
                }
            }
        }

        chunks
    }

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

    /// Calculate the total data size of a content tree by traversing stored nodes.
    fn calculate_tree_size(&self, root: H256) -> u64 {
        let mut size = 0u64;
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if let Some(node) = self.get_node(&hash) {
                if let Some(ref children) = node.children {
                    stack.extend(children.iter().copied());
                } else {
                    size = size.saturating_add(node.data.len() as u64);
                }
            }
        }

        size
    }
}

/// Hex encoding utility (simple implementation).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, &'static str> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        if s.len() % 2 != 0 {
            return Err("invalid hex length");
        }

        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex"))
            .collect()
    }
}

pub use hex::{decode as hex_decode, encode as hex_encode};

/// Build a balanced Merkle tree from leaf hashes, storing intermediate nodes in storage.
///
/// Pads to the next power of 2 with `H256::zero()`. Returns the tree root hash.
pub fn build_padded_merkle_tree(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    leaves: &[H256],
) -> H256 {
    if leaves.is_empty() {
        return H256::zero();
    }
    if leaves.len() == 1 {
        return leaves[0];
    }

    let padded_len = leaves.len().next_power_of_two();
    let mut current_level = leaves.to_vec();
    current_level.resize(padded_len, H256::zero());

    while current_level.len() > 1 {
        let mut next_level = Vec::new();
        for pair in current_level.chunks(2) {
            let parent = hash_children(pair[0], pair[1]);
            let mut node_data = Vec::new();
            node_data.extend_from_slice(pair[0].as_bytes());
            node_data.extend_from_slice(pair[1].as_bytes());
            let _ = storage.store_node(bucket_id, parent, node_data, Some(vec![pair[0], pair[1]]));
            next_level.push(parent);
        }
        current_level = next_level;
    }

    current_level[0]
}
