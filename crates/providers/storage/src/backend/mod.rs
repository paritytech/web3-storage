// SPDX-License-Identifier: Apache-2.0

//! Blob persistence layer: the [`StorageBackend`] trait and its implementations.
//!
//! [`StorageBackendSpec`] names an implementation and its configuration, and
//! builds it — that is what the provider node selects at startup.

pub mod rocksdb;
pub mod types;

pub use rocksdb::{DiskNonceStore, DiskStorage};
pub use types::{BucketState, DeletionReceipt, PrunedRange, StoredNode};

use crate::error::Error;
use crate::merkle::build_merkle_proof;
use crate::nonce::NonceStore;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use storage_primitives::{hash_children, BucketId, Commitment};

/// A built backend: the storage, and the nonce store matching its persistence.
pub type OpenedBackend = (Arc<dyn StorageBackend>, Arc<dyn NonceStore>);

/// Upper bound on the nodes one content-tree traversal visits.
///
/// Deduplication lets many parents reference one stored subtree, so the
/// logical node count of a tree is not bounded by the bytes stored under it.
/// This bound caps the work of `commit` and of chunk reads. At the default
/// chunk size it admits roughly 512 GiB under one data root.
pub const MAX_TREE_NODES: u64 = 1 << 22;

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

/// What `commit` produced: the commitment the provider signs, and the MMR
/// index of each committed data root, in input order.
#[derive(Debug)]
pub struct CommitOutcome {
    pub commitment: Commitment,
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

/// View of one pruned-but-not-yet-erased leaf range (the GC work queue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedRangeInfo {
    /// Global sequence number of the first stashed leaf.
    pub first_seq: u64,
    /// One past the last stashed leaf (`first_seq + len`).
    pub end_seq: u64,
    /// The start_seq the prune advanced the bucket to.
    pub new_start_seq: u64,
    /// Whether an admin-signed deletion receipt covering this range is held
    /// (required before the range may be physically erased).
    pub has_receipt: bool,
}

/// Result of physically erasing one pruned range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EraseOutcome {
    /// Nodes whose refcount reached zero and were deleted.
    pub nodes_deleted: u64,
    /// Bytes credited back to bucket quotas (sum over charged buckets).
    pub bytes_freed: u64,
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

    /// Commit data roots to the bucket's MMR. The returned commitment is the
    /// bucket state after the commit, read in the same transaction.
    fn commit(&self, bucket_id: BucketId, data_roots: Vec<H256>) -> Result<CommitOutcome, Error>;

    /// Node budget for one content-tree traversal; see [`MAX_TREE_NODES`].
    fn max_tree_nodes(&self) -> u64 {
        MAX_TREE_NODES
    }

    /// Visit every node under `root` in DFS order, once per path that reaches
    /// it, skipping `H256::zero()` padding. Fails with
    /// [`Error::NodeNotFound`] on a missing node and with
    /// [`Error::TreeTooLarge`] once the visit count passes
    /// [`Self::max_tree_nodes`].
    fn walk_tree(&self, root: H256, visit: &mut dyn FnMut(H256, &StoredNode)) -> Result<(), Error> {
        let max_nodes = self.max_tree_nodes();
        let mut visited = 0u64;
        let mut stack = vec![root];
        while let Some(hash) = stack.pop() {
            if hash == H256::zero() {
                continue;
            }
            visited += 1;
            if visited > max_nodes {
                return Err(Error::TreeTooLarge { max_nodes });
            }
            let node = self.get_node(&hash).ok_or_else(|| {
                Error::NodeNotFound(format!("0x{}", hex::encode(hash.as_bytes())))
            })?;
            if let Some(children) = &node.children {
                stack.extend(children.iter().rev().copied());
            }
            visit(hash, &node);
        }
        Ok(())
    }

    /// Collect chunk data under a data root, in logical order.
    fn collect_chunks(&self, root: H256) -> Result<Vec<Vec<u8>>, Error> {
        let mut chunks = Vec::new();
        self.walk_tree(root, &mut |_, node| {
            if node.children.is_none() {
                chunks.push(node.data.clone());
            }
        })?;
        Ok(chunks)
    }

    /// Collect chunk hashes under a data root, in logical order.
    fn collect_chunk_hashes(&self, root: H256) -> Result<Vec<H256>, Error> {
        let mut hashes = Vec::new();
        self.walk_tree(root, &mut |hash, node| {
            if node.children.is_none() {
                hashes.push(hash);
            }
        })?;
        Ok(hashes)
    }

    /// Chunk data and Merkle proofs for `range` of the logical chunk list
    /// under a data root, from one traversal. Indices past the end of the
    /// list are dropped.
    fn get_chunks_in_range(
        &self,
        data_root: H256,
        range: Range<u64>,
    ) -> Result<Vec<(Vec<u8>, storage_primitives::MerkleProof)>, Error> {
        let chunk_hashes = self.collect_chunk_hashes(data_root)?;
        let end = range.end.min(chunk_hashes.len() as u64);
        (range.start..end)
            .map(|chunk_index| {
                let chunk_hash = chunk_hashes[chunk_index as usize];
                let chunk_data = self
                    .get_node(&chunk_hash)
                    .ok_or_else(|| Error::NodeNotFound(format!("chunk_data_{chunk_index}")))?
                    .data;
                let proof = build_merkle_proof(&chunk_hashes, chunk_index as usize);
                Ok((chunk_data, proof))
            })
            .collect()
    }

    /// Get chunk data and Merkle proof at the given index from a data root.
    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, storage_primitives::MerkleProof), Error> {
        self.get_chunks_in_range(data_root, chunk_index..chunk_index.saturating_add(1))?
            .pop()
            .ok_or_else(|| Error::NodeNotFound(format!("chunk_{chunk_index}")))
    }

    /// Delete data before a sequence number.
    ///
    /// The pruned leaves are moved into a retention stash, not erased: the
    /// provider stays able to prove challenges against commitments covering
    /// them until an admin-signed deletion receipt is held and the canonical
    /// checkpoint has passed the range.
    fn delete_before(&self, bucket_id: BucketId, new_start_seq: u64) -> Result<Commitment, Error>;

    /// Store an admin-signed deletion receipt for a stashed range (matched
    /// by `new_start_seq`). Replaces a previous receipt for the same range.
    fn attach_deletion_receipt(
        &self,
        bucket_id: BucketId,
        receipt: DeletionReceipt,
    ) -> Result<(), Error>;

    /// The stored receipt with the smallest `new_start_seq` strictly greater
    /// than `seq` — the evidence defending a challenge on leaf `seq` after
    /// its bytes were erased.
    fn deletion_receipt_covering(&self, bucket_id: BucketId, seq: u64) -> Option<DeletionReceipt>;

    /// Set/refresh the bucket quota learned from the chain agreement.
    /// Never creates a bucket; errors if it does not exist.
    fn set_bucket_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error>;

    /// Pruned ranges awaiting physical erasure, oldest first.
    fn pruned_ranges(&self, bucket_id: BucketId) -> Vec<PrunedRangeInfo>;

    /// Whether the bucket was condemned (deleted on-chain / agreement gone).
    fn is_condemned(&self, bucket_id: BucketId) -> bool;

    /// Physically erase one stashed range: decrement refcounts along each
    /// leaf's tree, delete zero-ref nodes, credit `used_bytes` back to each
    /// node's charged bucket, and drop the range — one atomic write.
    /// Idempotent: an unknown `first_seq` is a no-op `Ok`. On a condemned
    /// bucket, removes the bucket row once nothing stashed or live remains.
    ///
    /// Callers are responsible for checking that liability has passed
    /// (canonical checkpoint past the range, the admin's deletion receipt
    /// held, no pending challenges).
    fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<EraseOutcome, Error>;

    /// Bucket teardown, first half: stash all remaining leaves as one pruned
    /// range and mark the bucket condemned. The second half is the caller
    /// (the GC) invoking [`erase_pruned_range`](Self::erase_pruned_range)
    /// once liability has passed — on a condemned bucket that also removes
    /// the bucket row itself. Idempotent; `Ok` if the bucket is already
    /// condemned or already gone.
    fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), Error>;

    /// Get MMR proof for a leaf.
    fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error>;

    /// Rebuild the MMR proof for the exact commitment a challenge cites.
    ///
    /// A challenge references a signed commitment's `(mmr_root, start_seq)`
    /// and a `leaf_index` relative to that `start_seq` — not the bucket's
    /// current state, which may have moved on through later commits or
    /// prunes. The proof must therefore be generated against the cited MMR
    /// state, reconstructed from the leaf history.
    fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: BucketId,
        commitment_root: H256,
        commitment_start_seq: u64,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, Error>;

    /// Get MMR peaks.
    fn get_mmr_peaks(&self, bucket_id: BucketId) -> Result<(H256, Vec<H256>), Error>;

    /// Logical size of the content under a data root: the sum of chunk sizes
    /// over every path, so a chunk referenced twice counts twice.
    fn calculate_tree_size(&self, root: H256) -> Result<u64, Error> {
        let mut size = 0u64;
        self.walk_tree(root, &mut |_, node| {
            if node.children.is_none() {
                size = size.saturating_add(node.data.len() as u64);
            }
        })?;
        Ok(size)
    }
}

/// Chunk `data` at [`storage_primitives::DEFAULT_CHUNK_SIZE`], store the
/// chunks and their Merkle tree, and commit the root to the bucket's MMR.
/// Creates the bucket with an unlimited quota if it does not exist. Returns
/// the data root and its leaf index.
pub fn commit_blob(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    data: &[u8],
) -> Result<(H256, u64), Error> {
    storage.init_bucket(bucket_id, u64::MAX)?;
    let chunks: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(storage_primitives::DEFAULT_CHUNK_SIZE as usize)
            .collect()
    };
    let chunk_hashes = chunks
        .iter()
        .map(|chunk| {
            let hash = storage_primitives::blake2_256(chunk);
            storage.store_node(bucket_id, hash, chunk.to_vec(), None)?;
            Ok(hash)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let data_root = build_padded_merkle_tree(storage, bucket_id, &chunk_hashes)?;
    let leaf_index = storage
        .commit(bucket_id, vec![data_root])?
        .leaf_indices
        .first()
        .copied()
        .ok_or(Error::RootNotFound(format!(
            "0x{}",
            hex::encode(data_root.as_bytes())
        )))?;
    Ok((data_root, leaf_index))
}

/// Build a balanced Merkle tree from leaf hashes, storing intermediate nodes in storage.
///
/// Pads to the next power of 2 with `H256::zero()`. Returns the tree root hash.
pub fn build_padded_merkle_tree(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    leaves: &[H256],
) -> Result<H256, Error> {
    if leaves.is_empty() {
        return Ok(H256::zero());
    }
    if leaves.len() == 1 {
        return Ok(leaves[0]);
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
            storage.store_node(bucket_id, parent, node_data, Some(vec![pair[0], pair[1]]))?;
            next_level.push(parent);
        }
        current_level = next_level;
    }

    Ok(current_level[0])
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

    /// Storing a blob under one data root must commit its full length as
    /// `data_size` (the signed leaf describes what was uploaded), an empty
    /// blob still yields one leaf, and each call appends one leaf.
    #[test]
    fn commit_blob_commits_body_length_and_appends_leaves() {
        let dir = TempDir::new().unwrap();
        let (storage, _nonce_store) = StorageBackendSpec::RocksDb {
            path: dir.path().to_path_buf(),
        }
        .build()
        .unwrap();
        let body = vec![7u8; storage_primitives::DEFAULT_CHUNK_SIZE as usize * 2 + 1];

        let (root, leaf_index) = commit_blob(storage.as_ref(), 1, &body).unwrap();
        let (_, empty_leaf_index) = commit_blob(storage.as_ref(), 1, &[]).unwrap();

        assert_eq!(leaf_index, 0);
        assert_eq!(empty_leaf_index, 1);
        assert_eq!(storage.collect_chunks(root).unwrap().concat(), body);
        assert_eq!(
            storage.calculate_tree_size(root).unwrap(),
            body.len() as u64
        );
        assert_eq!(storage.get_bucket(1).unwrap().leaf_count, 2);
    }
}
