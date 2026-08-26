// SPDX-License-Identifier: Apache-2.0

//! Blob persistence layer: the [`StorageBackend`] trait and its implementations.
//!
//! [`StorageBackendSpec`] names an implementation and its configuration, and
//! builds it — that is what the provider node selects at startup.

pub mod disk;

pub use disk::{DiskNonceStore, DiskStorage};

use crate::error::Error;
use crate::merkle::build_merkle_proof;
use crate::nonce::NonceStore;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use storage_primitives::{hash_children, BucketId};

/// A built backend: the storage, and the nonce store matching its persistence.
pub type OpenedBackend = (Arc<dyn StorageBackend>, Arc<dyn NonceStore>);

/// Which backend to build, and what that backend needs.
///
/// Each engine carries its own configuration, so adding one does not add a
/// sibling flag the others ignore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageBackendSpec {
    /// RocksDB rooted at `path`.
    RocksDb { path: PathBuf },
}

impl StorageBackendSpec {
    /// Build the backend and the nonce store matching its persistence, so the
    /// provider's extrinsic nonce survives a restart with its data.
    pub fn build(&self) -> Result<OpenedBackend, Error> {
        match self {
            Self::RocksDb { path } => {
                let disk = DiskStorage::new(path)?;
                let nonce_store = disk.nonce_store();
                Ok((Arc::new(disk), nonce_store))
            }
        }
    }
}

impl fmt::Display for StorageBackendSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RocksDb { path } => write!(f, "RocksDB at {}", path.display()),
        }
    }
}

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

/// Bucket summary info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSummary {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
}

/// Per-bucket statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketStats {
    pub bucket_id: BucketId,
    pub leaf_count: u64,
    pub node_count: u64,
    pub bytes_stored: u64,
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
    ///
    /// Returns `(mmr_root, start_seq, leaf_count, leaf_indices)`, all computed under
    /// the same lock so `leaf_count` is consistent with `mmr_root` (do not re-read it
    /// separately — a concurrent commit/delete could otherwise desync the two).
    fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<(H256, u64, u64, Vec<u64>), Error>;

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

    /// Rebuild the MMR proof for the exact commitment a challenge cites.
    ///
    /// A challenge references a signed commitment's `(mmr_root, start_seq,
    /// leaf_count)` and a `leaf_index` relative to that `start_seq` — not the
    /// bucket's current state, which may have moved on through later commits
    /// or prunes. The proof must therefore be generated against the cited MMR
    /// state, reconstructed from that exact prefix of the leaf history.
    fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment_root: H256,
        commitment_start_seq: u64,
        commitment_leaf_count: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn rocksdb_pairs_with_a_nonce_store_that_survives_reopen() {
        let dir = TempDir::new().unwrap();
        let spec = StorageBackendSpec::RocksDb {
            path: dir.path().to_path_buf(),
        };

        // Scoped so both halves drop and RocksDB releases the directory lock.
        {
            let (_storage, nonce_store) = spec.build().expect("RocksDB opens");
            nonce_store.persist(7);
        }

        let (_storage, nonce_store) = spec.build().expect("RocksDB reopens");
        assert_eq!(nonce_store.load(), Some(7));
        assert!(spec.to_string().starts_with("RocksDB at "));
    }
}
