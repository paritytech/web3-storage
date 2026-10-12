// SPDX-License-Identifier: Apache-2.0

//! Blob persistence layer: the [`StorageBackend`] trait and its implementations.
//!
//! [`StorageBackendSpec`] names an implementation and its configuration, and
//! builds it — that is what the provider node selects at startup.

pub mod rocksdb;
pub mod types;

pub use rocksdb::DiskStorage;
pub use types::{BucketState, StoredNode};

use crate::error::Error;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use storage_primitives::BucketId;

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
    /// Build the backend.
    pub fn build(&self) -> Result<Arc<dyn StorageBackend>, Error> {
        match self {
            Self::RocksDb { path } => Ok(Arc::new(DiskStorage::new(path)?)),
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

/// Storage engine interface. Callers hold an `Arc<dyn StorageBackend>` rather
/// than a concrete engine.
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

        let proof = storage_primitives::padded_merkle_proof(&chunk_hashes, chunk_index as usize)
            .ok_or_else(|| Error::NodeNotFound(format!("chunk_{chunk_index}")))?;

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

/// Build a balanced Merkle tree from leaf hashes, storing intermediate nodes in storage.
///
/// The tree shape is `storage_primitives::padded_merkle_tree`. Returns the tree root hash.
pub fn build_padded_merkle_tree(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    leaves: &[H256],
) -> H256 {
    let (root, nodes) = storage_primitives::padded_merkle_tree(leaves);
    for node in nodes {
        let _ = storage.store_node(
            bucket_id,
            node.hash,
            node.data().to_vec(),
            Some(vec![node.left, node.right]),
        );
    }
    root
}
