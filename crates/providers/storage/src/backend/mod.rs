// SPDX-License-Identifier: Apache-2.0

//! Blob persistence layer: the [`StorageBackend`] trait and its implementations.
//!
//! [`StorageBackendSpec`] names an implementation and its configuration, and
//! builds it — that is what the provider node selects at startup.

pub mod rocksdb;
pub mod types;

pub use rocksdb::DiskStorage;
pub use types::{BucketState, ChunkTreeNode};

use crate::error::Error;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use storage_primitives::{
    build_merkle_proofs, merkle_root, merkle_tree, BucketId, DEFAULT_CHUNK_SIZE,
};

/// A chunk's hash, its data and its Merkle proof (sibling hashes up to the
/// data root).
pub type ProvenChunk = (H256, Vec<u8>, Vec<H256>);

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

/// Result of committing data roots to a bucket's MMR.
///
/// `mmr_root` and `leaf_count` come from the same bucket state, so they are
/// consistent with each other; a separate read could see a later commit or
/// delete.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// MMR root after the commit.
    pub mmr_root: H256,
    /// First live sequence number of the bucket's window.
    pub start_seq: u64,
    /// Leaf count backing `mmr_root`.
    pub leaf_count: u64,
    /// Leaf indices of the committed leaves within this commitment (relative to
    /// `start_seq`).
    pub leaf_indices: Vec<u64>,
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

    /// Store a node under the hash the caller claims for it.
    fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        node: ChunkTreeNode,
    ) -> Result<(), Error>;

    /// Get a node by hash.
    fn get_node(&self, hash: &H256) -> Option<ChunkTreeNode>;

    /// Check which hashes exist in storage.
    fn check_exists(&self, bucket_id: BucketId, hashes: &[H256]) -> (Vec<H256>, Vec<H256>);

    /// Commit data roots to the bucket's MMR.
    fn commit(&self, bucket_id: BucketId, data_roots: Vec<H256>) -> Result<CommitOutcome, Error>;

    /// Collect actual chunk data under a data root (DFS, leaf data in order).
    fn collect_chunks(&self, root: H256) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if let Some(node) = self.get_node(&hash) {
                match node {
                    ChunkTreeNode::Internal { left, right } => {
                        stack.push(right);
                        stack.push(left);
                    }
                    ChunkTreeNode::Chunk(data) => chunks.push(data),
                }
            }
        }

        chunks
    }

    /// Hash and byte size of each chunk under `root`, in leaf order. Skips
    /// nodes missing from storage.
    fn collect_leaves(&self, root: H256) -> Vec<(H256, usize)> {
        let mut leaves = Vec::new();
        let mut stack = vec![root];

        while let Some(hash) = stack.pop() {
            if let Some(node) = self.get_node(&hash) {
                match node {
                    ChunkTreeNode::Internal { left, right } => {
                        stack.push(right);
                        stack.push(left);
                    }
                    ChunkTreeNode::Chunk(data) => leaves.push((hash, data.len())),
                }
            }
        }

        leaves
    }

    /// Byte size of the data under `root`. Fails with `NonCanonicalTree` unless
    /// `root` is `merkle_root` over chunks of `DEFAULT_CHUNK_SIZE` bytes, except
    /// the last, which has 1 to `DEFAULT_CHUNK_SIZE` bytes.
    fn canonical_data_size(&self, root: H256) -> Result<u64, Error> {
        let (chunk_hashes, chunk_sizes): (Vec<H256>, Vec<usize>) =
            self.collect_leaves(root).into_iter().unzip();
        let chunk_size = DEFAULT_CHUNK_SIZE as usize;
        let fixed_sizes = match chunk_sizes.split_last() {
            None => false,
            Some((last, rest)) => {
                rest.iter().all(|size| *size == chunk_size) && (1..=chunk_size).contains(last)
            }
        };
        if !fixed_sizes || merkle_root(&chunk_hashes) != root {
            return Err(Error::NonCanonicalTree(format!("{:#x}", root)));
        }
        Ok(chunk_sizes.iter().map(|size| *size as u64).sum())
    }

    /// Chunk data and Merkle proof at `chunk_index` under `data_root`.
    /// `ChunkIndexOutOfRange` when the tree has no chunk at that index.
    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<H256>), Error> {
        let chunk_hashes = chunk_hashes(self, data_root);
        let chunk_count = chunk_hashes.len() as u64;
        if chunk_index >= chunk_count {
            return Err(Error::ChunkIndexOutOfRange {
                chunk_index,
                chunk_count,
            });
        }
        let (_, data, proof) = proven_chunks(self, &chunk_hashes, chunk_index..chunk_index + 1)?
            .pop()
            .ok_or_else(|| Error::NodeNotFound(format!("chunk_{chunk_index}")))?;
        Ok((data, proof))
    }

    /// Hash, data and Merkle proof of each chunk of `data_root` with an index
    /// in `range`. Leaves out indices past the last chunk.
    fn get_chunks_in_range(
        &self,
        data_root: H256,
        range: Range<u64>,
    ) -> Result<Vec<ProvenChunk>, Error> {
        proven_chunks(self, &chunk_hashes(self, data_root), range)
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
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error>;

    /// Get MMR peaks.
    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error>;
}

/// Leaf hashes of the tree under `data_root`, in chunk order.
fn chunk_hashes<B: StorageBackend + ?Sized>(backend: &B, data_root: H256) -> Vec<H256> {
    backend
        .collect_leaves(data_root)
        .into_iter()
        .map(|(hash, _)| hash)
        .collect()
}

/// Hash, data and Merkle proof of the chunks of `chunk_hashes` with an index
/// in `range`, clipped to the tree.
fn proven_chunks<B: StorageBackend + ?Sized>(
    backend: &B,
    chunk_hashes: &[H256],
    range: Range<u64>,
) -> Result<Vec<ProvenChunk>, Error> {
    let chunk_count = chunk_hashes.len() as u64;
    let start = range.start.min(chunk_count) as usize;
    let proofs = build_merkle_proofs(chunk_hashes, start..range.end.min(chunk_count) as usize);

    chunk_hashes[start..]
        .iter()
        .zip(proofs)
        .enumerate()
        .map(|(i, (hash, proof))| match backend.get_node(hash) {
            Some(ChunkTreeNode::Chunk(data)) => Ok((*hash, data, proof)),
            _ => Err(Error::NodeNotFound(format!("chunk_data_{}", start + i))),
        })
        .collect()
}

/// Store the internal nodes of `storage_primitives::merkle_tree` over `leaves`
/// and return the root. Fails with `EmptyData` for no leaves, and otherwise
/// returns the first `store_node` error.
pub fn build_merkle_tree(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    leaves: &[H256],
) -> Result<H256, Error> {
    if leaves.is_empty() {
        return Err(Error::EmptyData);
    }
    let (root, nodes) = merkle_tree(leaves);
    for node in nodes {
        storage.store_node(
            bucket_id,
            node.hash,
            ChunkTreeNode::Internal {
                left: node.left,
                right: node.right,
            },
        )?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::{hash_leaf, verify_merkle_proof};
    use tempfile::TempDir;

    /// A database with bucket 1 of `quota` bytes that stores `chunks`, and the
    /// chunk hashes.
    fn storage_with_chunks(quota: u64, chunks: &[Vec<u8>]) -> (TempDir, DiskStorage, Vec<H256>) {
        let dir = TempDir::new().unwrap();
        let storage = DiskStorage::new(dir.path()).unwrap();
        storage.init_bucket(1, quota).unwrap();
        let hashes = chunks
            .iter()
            .map(|data| {
                let hash = hash_leaf(data);
                storage
                    .store_node(1, hash, ChunkTreeNode::Chunk(data.clone()))
                    .unwrap();
                hash
            })
            .collect();
        (dir, storage, hashes)
    }

    fn chunks(n: u8) -> Vec<Vec<u8>> {
        (1..=n).map(|i| vec![i; 32]).collect()
    }

    #[test]
    fn stored_tree_root_matches_merkle_root() {
        for n in [1, 2, 3, 5, 8] {
            let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &chunks(n));
            assert_eq!(
                build_merkle_tree(&storage, 1, &leaves).unwrap(),
                storage_primitives::merkle_root(&leaves),
                "{n} leaves"
            );
        }
    }

    #[test]
    fn chunks_in_range_match_single_chunks_and_clip_to_the_last_chunk() {
        let chunks = chunks(5);
        let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &chunks);
        let root = build_merkle_tree(&storage, 1, &leaves).unwrap();

        let range = storage.get_chunks_in_range(root, 1..4).unwrap();
        assert_eq!(range.len(), 3);
        for (index, (hash, data, proof)) in (1..4).zip(&range) {
            assert_eq!(*data, chunks[index as usize]);
            assert_eq!(*hash, leaves[index as usize]);
            assert!(verify_merkle_proof(*hash, index, 5, proof, &root));
            assert_eq!(
                *proof,
                storage.get_chunk_at_index(root, index).unwrap().1,
                "index {index}"
            );
        }

        let clipped: Vec<Vec<u8>> = storage
            .get_chunks_in_range(root, 3..10)
            .unwrap()
            .into_iter()
            .map(|(_, data, _)| data)
            .collect();
        assert_eq!(clipped, chunks[3..]);
        assert!(storage.get_chunks_in_range(root, 7..9).unwrap().is_empty());
        let reversed = Range { start: 3, end: 1 };
        assert!(storage
            .get_chunks_in_range(root, reversed)
            .unwrap()
            .is_empty());
        assert!(matches!(
            storage.get_chunk_at_index(root, 5),
            Err(Error::ChunkIndexOutOfRange {
                chunk_index: 5,
                chunk_count: 5
            })
        ));
    }

    #[test]
    fn build_merkle_tree_rejects_no_leaves() {
        let (_dir, storage, _) = storage_with_chunks(1024, &[]);
        let err = build_merkle_tree(&storage, 1, &[]).unwrap_err();
        assert!(matches!(err, Error::EmptyData), "got {err:?}");
    }

    #[test]
    fn build_merkle_tree_returns_store_error() {
        // Room for the two 32-byte leaves but not for their 64-byte parent.
        let (_dir, storage, leaves) = storage_with_chunks(64, &chunks(2));
        let err = build_merkle_tree(&storage, 1, &leaves).unwrap_err();
        assert!(matches!(err, Error::QuotaExceeded { .. }), "got {err:?}");
    }
}
