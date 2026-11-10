// SPDX-License-Identifier: Apache-2.0

//! Blob persistence layer: the [`StorageBackend`] trait and its implementations.
//!
//! [`StorageBackendSpec`] names an implementation and its configuration, and
//! builds it — that is what the provider node selects at startup.

pub mod rocksdb;
pub mod types;

pub use rocksdb::DiskStorage;
pub use types::{BucketState, ChunkTreeNode, DeletionReceipt, PrunedRange};

use crate::error::Error;
use sp_core::H256;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use storage_primitives::{merkle_root, BucketId, Commitment, DEFAULT_CHUNK_SIZE};

/// A chunk's hash, its data and its Merkle proof (sibling hashes up to the
/// data root).
pub type ProvenChunk = (H256, Vec<u8>, Vec<H256>);

/// Upper bound on the nodes one content-tree traversal visits.
///
/// Deduplication lets many parents reference one stored subtree, so the
/// logical node count of a tree is not bounded by the bytes stored under it.
/// This bound caps the work of `commit`. At the default
/// chunk size it admits roughly 512 GiB under one data root.
pub const MAX_TREE_WALK_VISITS: u64 = 1 << 22;

/// Deepest chunk tree a read follows. A tree of depth `d` spans up to `2^d`
/// chunks, and chunk counts and indices are `u64`, so a depth of 64 or more
/// cannot belong to a valid tree.
pub const MAX_TREE_DEPTH: u32 = 63;

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

/// Totals across every bucket a backend holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    /// Number of buckets.
    pub buckets: u64,
    /// Nodes stored, each counted once.
    pub nodes: u64,
    /// Bytes of node data charged against bucket quotas.
    pub bytes: u64,
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

/// Garbage collector state a backend persists across restarts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GarbageCollectorConfiguration {
    /// Highest finalized block number the garbage collector has read chain
    /// state at; 0 if none.
    pub last_scanned_block: u64,
}

/// Storage engine interface. Callers hold an `Arc<dyn StorageBackend>` rather
/// than a concrete engine.
pub trait StorageBackend: Send + Sync {
    /// Initialize a bucket with the given quota.
    fn init_bucket(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error>;

    /// Get bucket information.
    ///
    /// `Ok(None)` means the bucket genuinely does not exist. `Err` means the
    /// backend itself failed - the bucket may still be there.
    fn get_bucket(&self, bucket_id: BucketId) -> Result<Option<BucketInfo>, Error>;

    /// Ids of every locally-held bucket.
    fn bucket_ids(&self) -> Vec<BucketId>;

    /// Totals across all buckets, read from the per-bucket counters.
    fn stats(&self) -> StorageStats;

    /// Store a node under the hash the caller claims for it. Fails with
    /// [`Error::BucketCondemned`] on a condemned bucket.
    fn store_node(
        &self,
        bucket_id: BucketId,
        expected_hash: H256,
        node: ChunkTreeNode,
    ) -> Result<(), Error>;

    /// Get a node by hash.
    ///
    /// `Ok(None)` means the node genuinely does not exist. `Err` means the
    /// backend itself failed - the node may still be there.
    fn get_node(&self, hash: &H256) -> Result<Option<ChunkTreeNode>, Error>;

    /// Split `hashes` into those stored and those missing. `Err` means a
    /// read failed, not that a node is absent.
    fn check_exists(
        &self,
        bucket_id: BucketId,
        hashes: &[H256],
    ) -> Result<(Vec<H256>, Vec<H256>), Error>;

    /// Commit data roots to the bucket's MMR. Fails with
    /// [`Error::BucketCondemned`] on a condemned bucket.
    fn commit(&self, bucket_id: BucketId, data_roots: Vec<H256>) -> Result<CommitOutcome, Error>;

    /// Node budget for one content-tree traversal; see [`MAX_TREE_WALK_VISITS`].
    fn max_tree_walk_visits(&self) -> u64 {
        MAX_TREE_WALK_VISITS
    }

    /// Visit every node under `root` in DFS order, once per path that reaches
    /// it. Fails with [`Error::NodeNotFound`] on a missing node and with
    /// [`Error::TreeTooLarge`] once the visit count passes
    /// [`Self::max_tree_walk_visits`].
    fn walk_tree(
        &self,
        root: H256,
        visit: &mut dyn FnMut(H256, &ChunkTreeNode),
    ) -> Result<(), Error> {
        let max_nodes = self.max_tree_walk_visits();
        let mut visited = 0u64;
        let mut stack = vec![root];
        while let Some(hash) = stack.pop() {
            visited += 1;
            if visited > max_nodes {
                return Err(Error::TreeTooLarge { max_nodes });
            }
            let node = self.get_node(&hash)?.ok_or(Error::NodeNotFound(hash))?;
            if let ChunkTreeNode::Internal { left, right } = node {
                stack.push(right);
                stack.push(left);
            }
            visit(hash, &node);
        }
        Ok(())
    }

    /// Visit every node under `root`, a canonical tree of `chunk_count`
    /// chunks, once per path that reaches it, in DFS order.
    ///
    /// A canonical tree over `n > 1` chunks puts the largest power of two
    /// below `n` on the left and the rest on the right, so its shape follows
    /// from `chunk_count`. A node that spans one chunk is a chunk: the walk
    /// visits it by hash and does not read it. Only internal nodes are read.
    /// Fails like [`Self::walk_tree`] on a missing internal node or a visit
    /// count past the budget, and with [`Error::NonCanonicalTree`] when a
    /// node that spans more than one chunk is not internal.
    fn walk_canonical_tree(
        &self,
        root: H256,
        chunk_count: u64,
        visit: &mut dyn FnMut(H256),
    ) -> Result<(), Error> {
        let max_nodes = self.max_tree_walk_visits();
        let mut visited = 0u64;
        let mut stack = vec![(root, chunk_count)];
        while let Some((hash, count)) = stack.pop() {
            visited += 1;
            if visited > max_nodes {
                return Err(Error::TreeTooLarge { max_nodes });
            }
            if count > 1 {
                let ChunkTreeNode::Internal { left, right } = node_at(self, &hash)? else {
                    return Err(Error::NonCanonicalTree(format!("{:#x}", root)));
                };
                let split = 1u64 << (count - 1).ilog2();
                stack.push((right, count - split));
                stack.push((left, split));
            }
            visit(hash);
        }
        Ok(())
    }

    /// Hash and byte size of each chunk under `root`, in leaf order.
    ///
    /// A missing node or a read failure must not silently shorten the result:
    /// this backs [`Self::get_chunk_at_index`], the challenge proof path,
    /// where a shortened list would make a transient backend failure
    /// indistinguishable from the chunk actually being missing.
    fn collect_leaves(&self, root: H256) -> Result<Vec<(H256, usize)>, Error> {
        let mut leaves = Vec::new();
        self.walk_tree(root, &mut |hash, node| {
            if let ChunkTreeNode::Chunk(data) = node {
                leaves.push((hash, data.len()));
            }
        })?;
        Ok(leaves)
    }

    /// Byte size of the data under `root`; see [`canonical_size`].
    fn canonical_data_size(&self, root: H256) -> Result<u64, Error> {
        canonical_size(root, &self.collect_leaves(root)?)
    }

    /// Chunk data and Merkle proof at `chunk_index` under `data_root`.
    /// `ChunkIndexOutOfRange` when the tree has no chunk at that index.
    fn get_chunk_at_index(
        &self,
        data_root: H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<H256>), Error> {
        let chunk_count = chunk_count(self, data_root)?;
        if chunk_index >= chunk_count {
            return Err(Error::ChunkIndexOutOfRange {
                chunk_index,
                chunk_count,
            });
        }
        let (_, data, proof) = chunk_with_proof(self, data_root, chunk_count, chunk_index)?;
        Ok((data, proof))
    }

    /// Hash, data and Merkle proof of each chunk of `data_root` with an index
    /// in `range`. Leaves out indices past the last chunk. An unknown
    /// `data_root` has no chunks.
    fn get_chunks_in_range(
        &self,
        data_root: H256,
        range: Range<u64>,
    ) -> Result<Vec<ProvenChunk>, Error> {
        let chunk_count = chunk_count(self, data_root)?;
        (range.start..range.end.min(chunk_count))
            .map(|index| chunk_with_proof(self, data_root, chunk_count, index))
            .collect()
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
    /// A condemned bucket rejects it with [`Error::BucketCondemned`], and a
    /// receipt whose signature does not verify with
    /// [`Error::InvalidDeletionReceipt`].
    fn attach_deletion_receipt(
        &self,
        bucket_id: BucketId,
        receipt: DeletionReceipt,
    ) -> Result<(), Error>;

    /// The stored receipt with the smallest `new_start_seq` strictly greater
    /// than `seq` — the evidence defending a challenge on leaf `seq` after
    /// its bytes were erased.
    fn deletion_receipt_covering(
        &self,
        bucket_id: BucketId,
        seq: u64,
    ) -> Result<Option<DeletionReceipt>, Error>;

    /// Set/refresh the bucket quota learned from the chain agreement.
    /// Never creates a bucket; errors if it does not exist.
    fn set_bucket_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error>;

    /// Pruned ranges awaiting physical erasure, oldest first.
    fn pruned_ranges(&self, bucket_id: BucketId) -> Result<Vec<PrunedRangeInfo>, Error>;

    /// Whether the bucket was condemned (deleted on-chain / agreement gone).
    fn is_condemned(&self, bucket_id: BucketId) -> Result<bool, Error>;

    /// The stored garbage collector configuration; every field is its
    /// default when nothing was stored. Persists across restarts.
    fn garbage_collector_configuration(&self) -> Result<GarbageCollectorConfiguration, Error>;

    /// Replace the stored garbage collector configuration.
    fn set_garbage_collector_configuration(
        &self,
        configuration: &GarbageCollectorConfiguration,
    ) -> Result<(), Error>;

    /// Physically erase one stashed range: decrement refcounts along each
    /// leaf's tree, delete zero-ref nodes, credit `used_bytes` back to each
    /// node's charged bucket, and count the leaves erased. The work runs in
    /// batches of whole leaves, each batch one atomic write; a failure keeps
    /// the batches already applied and a later call resumes after them. The
    /// range's leaves remain (see [`PrunedRange`]); the range record goes
    /// once every leaf is erased. Idempotent: an unknown or already erased
    /// `first_seq` is a no-op `Ok`. On a condemned bucket, removes the
    /// bucket's records once no live leaf and no unerased range remains.
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
    /// the bucket's records. Idempotent; `Ok` if the bucket is already
    /// condemned or already gone. On a condemned bucket with nothing left to
    /// erase it resumes a removal that stopped part way. A condemned bucket
    /// rejects `store_node` and `commit` with [`Error::BucketCondemned`].
    fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), Error>;

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

/// Byte size of the chunks `leaves` (hash and byte size, in leaf order) under
/// `root`. Fails with `NonCanonicalTree` unless `root` is `merkle_root` over
/// chunks of `DEFAULT_CHUNK_SIZE` bytes, except the last, which has 1 to
/// `DEFAULT_CHUNK_SIZE` bytes.
pub(crate) fn canonical_size(root: H256, leaves: &[(H256, usize)]) -> Result<u64, Error> {
    let (chunk_hashes, chunk_sizes): (Vec<H256>, Vec<usize>) = leaves.iter().copied().unzip();
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

fn node_at<B: StorageBackend + ?Sized>(backend: &B, hash: &H256) -> Result<ChunkTreeNode, Error> {
    backend.get_node(hash)?.ok_or(Error::NodeNotFound(*hash))
}

/// Number of chunks under `root`, from the nodes down its right edge.
///
/// In the chunk tree the left child of a node spans a power of two of chunks,
/// so each node on the right edge adds the size of its left child, and the
/// last chunk adds one. An edge deeper than [`MAX_TREE_DEPTH`] is not a chunk
/// tree.
fn chunk_count<B: StorageBackend + ?Sized>(backend: &B, root: H256) -> Result<u64, Error> {
    let mut count = 0u64;
    let mut hash = root;
    for _ in 0..=MAX_TREE_DEPTH {
        match node_at(backend, &hash)? {
            ChunkTreeNode::Chunk(_) => {
                return count
                    .checked_add(1)
                    .ok_or_else(|| Error::NonCanonicalTree(format!("{:#x}", root)))
            }
            ChunkTreeNode::Internal { left, right } => {
                count = count
                    .checked_add(left_child_chunks(backend, left, root)?)
                    .ok_or_else(|| Error::NonCanonicalTree(format!("{:#x}", root)))?;
                hash = right;
            }
        }
    }
    Err(Error::NonCanonicalTree(format!("{:#x}", root)))
}

/// Chunks under `left`, a left child: `2^h` for `h` nodes down its left edge.
fn left_child_chunks<B: StorageBackend + ?Sized>(
    backend: &B,
    left: H256,
    root: H256,
) -> Result<u64, Error> {
    let mut hash = left;
    for height in 0..=MAX_TREE_DEPTH {
        match node_at(backend, &hash)? {
            ChunkTreeNode::Chunk(_) => return Ok(1u64 << height),
            ChunkTreeNode::Internal { left, .. } => hash = left,
        }
    }
    Err(Error::NonCanonicalTree(format!("{:#x}", root)))
}

/// Hash, data and Merkle proof of chunk `index` in the tree of `chunk_count`
/// chunks under `root`. A tree over `n > 1` chunks splits into the largest
/// power of two below `n` on the left and the rest on the right, and the
/// proof lists the sibling at each split, from the chunk up.
fn chunk_with_proof<B: StorageBackend + ?Sized>(
    backend: &B,
    root: H256,
    chunk_count: u64,
    index: u64,
) -> Result<ProvenChunk, Error> {
    let (mut hash, mut count, mut index) = (root, chunk_count, index);
    let mut siblings = Vec::new();
    loop {
        match node_at(backend, &hash)? {
            ChunkTreeNode::Chunk(data) if count == 1 => {
                siblings.reverse();
                return Ok((hash, data, siblings));
            }
            ChunkTreeNode::Internal { left, right } if count > 1 => {
                let split = 1u64 << (count - 1).ilog2();
                if index < split {
                    siblings.push(right);
                    (hash, count) = (left, split);
                } else {
                    siblings.push(left);
                    (hash, count, index) = (right, count - split, index - split);
                }
            }
            _ => return Err(Error::NonCanonicalTree(format!("{:#x}", root))),
        }
    }
}

/// Store the internal nodes of `storage_primitives::merkle_tree` over
/// `leaves` (at least one) and return the root, or the first `store_node`
/// error. Test helper: clients upload internal nodes through `PUT /node`.
#[cfg(any(test, feature = "test-helpers"))]
pub fn build_merkle_tree(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    leaves: &[H256],
) -> Result<H256, Error> {
    assert!(!leaves.is_empty(), "a Merkle tree needs at least one leaf");
    let (root, nodes) = storage_primitives::merkle_tree(leaves);
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
    use storage_primitives::{build_merkle_proofs, hash_leaf, verify_merkle_proof};
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

    /// Reads return the chunks and the proofs `build_merkle_proofs` builds
    /// for every chunk count up to 70, for every index and for ranges that
    /// reach past the end or run backwards.
    #[test]
    fn reads_match_build_merkle_proofs_for_every_chunk_count() {
        let chunks = chunks(70);
        let (_dir, storage, all_leaves) = storage_with_chunks(u64::MAX, &chunks);
        for n in 1..=70usize {
            let leaves = &all_leaves[..n];
            let root = build_merkle_tree(&storage, 1, leaves).unwrap();

            for index in 0..n {
                let (data, proof) = storage.get_chunk_at_index(root, index as u64).unwrap();
                assert_eq!(data, chunks[index], "n {n} index {index}");
                assert_eq!(
                    proof,
                    build_merkle_proofs(leaves, index..index + 1).remove(0),
                    "n {n} index {index}"
                );
                assert!(verify_merkle_proof(
                    leaves[index],
                    index as u64,
                    n as u64,
                    &proof,
                    &root
                ));
            }
            assert!(matches!(
                storage.get_chunk_at_index(root, n as u64),
                Err(Error::ChunkIndexOutOfRange { chunk_index, chunk_count })
                    if chunk_index == n as u64 && chunk_count == n as u64
            ));

            for (start, end) in [
                (0, n),
                (1, n),
                (0, 1),
                (n - 1, n + 3),
                (n, n + 2),
                (n + 5, n + 9),
                (3, 1),
            ] {
                let (lo, hi) = (start.min(n), end.min(n));
                let expected: Vec<ProvenChunk> = if lo >= hi {
                    Vec::new()
                } else {
                    (lo..hi)
                        .zip(build_merkle_proofs(leaves, lo..hi))
                        .map(|(i, proof)| (leaves[i], chunks[i].clone(), proof))
                        .collect()
                };
                let got = storage
                    .get_chunks_in_range(root, start as u64..end as u64)
                    .unwrap();
                assert_eq!(got, expected, "n {n} range {start}..{end}");
            }
        }
    }

    #[test]
    fn build_merkle_tree_returns_store_error() {
        // Room for the two 32-byte leaves but not for their 64-byte parent.
        let (_dir, storage, leaves) = storage_with_chunks(64, &chunks(2));
        let err = build_merkle_tree(&storage, 1, &leaves).unwrap_err();
        assert!(matches!(err, Error::QuotaExceeded { .. }), "got {err:?}");
    }
}
