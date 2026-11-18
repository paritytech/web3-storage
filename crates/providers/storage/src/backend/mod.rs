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
use std::path::PathBuf;
use std::sync::Arc;
use storage_primitives::{merkle_root, BucketId, Commitment, DEFAULT_CHUNK_SIZE};

/// A chunk's hash, its data and its Merkle proof (sibling hashes up to the
/// data root).
pub type ProvenChunk = (H256, Vec<u8>, Vec<H256>);

/// Result of [`StorageBackend::read_chunks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRead {
    /// Index of the first chunk in `chunks`.
    pub start_chunk: u64,
    /// Number of chunks under the data root.
    pub chunk_count: u64,
    /// The chunks read, in index order.
    pub chunks: Vec<ProvenChunk>,
}

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

/// What a tree walk needs to know about a node: a chunk's byte size, or an
/// internal node's children. Reading it does not read a chunk's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    /// A chunk of `size` bytes.
    Chunk { size: u64 },
    /// An internal node over two children.
    Internal { left: H256, right: H256 },
}

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

    /// Store a node under the hash the caller claims for it.
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

    /// Get a node's shape by hash, without reading a chunk's data.
    ///
    /// `Ok(None)` means the node genuinely does not exist. `Err` means the
    /// backend itself failed - the node may still be there.
    fn node_shape(&self, hash: &H256) -> Result<Option<NodeShape>, Error>;

    /// Split `hashes` into those stored and those missing. `Err` means a
    /// read failed, not that a node is absent.
    fn check_exists(
        &self,
        bucket_id: BucketId,
        hashes: &[H256],
    ) -> Result<(Vec<H256>, Vec<H256>), Error>;

    /// Commit data roots to the bucket's MMR.
    fn commit(&self, bucket_id: BucketId, data_roots: Vec<H256>) -> Result<CommitOutcome, Error>;

    /// Node budget for one content-tree traversal; see [`MAX_TREE_WALK_VISITS`].
    fn max_tree_walk_visits(&self) -> u64 {
        MAX_TREE_WALK_VISITS
    }

    /// Visit every node under `root` in DFS order, once per path that reaches
    /// it. Fails with [`Error::NodeNotFound`] on a missing node and with
    /// [`Error::TreeTooLarge`] once the visit count passes
    /// [`Self::max_tree_walk_visits`]. A chunk is visited by its size and is
    /// not read.
    fn walk_tree(&self, root: H256, visit: &mut dyn FnMut(H256, &NodeShape)) -> Result<(), Error> {
        let max_nodes = self.max_tree_walk_visits();
        let mut visited = 0u64;
        let mut stack = vec![root];
        while let Some(hash) = stack.pop() {
            visited += 1;
            if visited > max_nodes {
                return Err(Error::TreeTooLarge { max_nodes });
            }
            let shape = self.node_shape(&hash)?.ok_or(Error::NodeNotFound(hash))?;
            if let NodeShape::Internal { left, right } = shape {
                stack.push(right);
                stack.push(left);
            }
            visit(hash, &shape);
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
        self.walk_tree(root, &mut |hash, shape| {
            if let NodeShape::Chunk { size } = shape {
                leaves.push((hash, *size as usize));
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
        let (hash, proof) = locate_chunk(self, data_root, chunk_count, chunk_index)?;
        Ok((chunk_data(self, data_root, &hash)?, proof))
    }

    /// Chunks of `data_root` from index `start`, with their proofs. Stops
    /// after `max_chunks` chunks, at the last chunk, or before the data passes
    /// `max_bytes`, but returns at least one chunk.
    fn read_chunks(
        &self,
        data_root: H256,
        start: u64,
        max_chunks: u64,
        max_bytes: u64,
    ) -> Result<ChunkRead, Error> {
        let chunk_count = chunk_count(self, data_root)?;
        read_chunks_of(self, data_root, chunk_count, start, max_chunks, max_bytes)
    }

    /// Whole chunks of `data_root` that cover `offset..offset + length`, with
    /// the limits of [`Self::read_chunks`]. The chunk size is the size of the
    /// first chunk.
    fn read_range(
        &self,
        data_root: H256,
        offset: u64,
        length: u64,
        max_chunks: u64,
        max_bytes: u64,
    ) -> Result<ChunkRead, Error> {
        let chunk_count = chunk_count(self, data_root)?;
        let chunk_size = first_chunk_size(self, data_root)?;
        if chunk_size == 0 && chunk_count > 1 {
            return Err(Error::NonCanonicalTree(format!("{:#x}", data_root)));
        }
        let chunk_size = chunk_size.max(1);
        let start = offset / chunk_size;
        let end = match length {
            0 => start,
            _ => offset.saturating_add(length).div_ceil(chunk_size),
        };
        read_chunks_of(
            self,
            data_root,
            chunk_count,
            start,
            end.saturating_sub(start).min(max_chunks),
            max_bytes,
        )
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
    fn deletion_receipt_covering(
        &self,
        bucket_id: BucketId,
        seq: u64,
    ) -> Result<Option<DeletionReceipt>, Error>;

    /// Set the quota of `bucket_id`, creating the bucket if it does not exist.
    /// The check and the write are atomic.
    fn init_bucket_or_set_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), Error>;

    /// Pruned ranges awaiting physical erasure, oldest first.
    fn pruned_ranges(&self, bucket_id: BucketId) -> Result<Vec<PrunedRangeInfo>, Error>;

    /// Whether the bucket was condemned (deleted on-chain / agreement gone).
    fn is_condemned(&self, bucket_id: BucketId) -> Result<bool, Error>;

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

    /// Build the MMR proof of leaf `leaf_index`, counted from
    /// `commitment.start_seq`, in the MMR of `commitment`.
    ///
    /// The bucket's current state may have moved past `commitment` through
    /// later commits or prunes, so the proof is built from the commitment's
    /// range of the leaf history, and the rebuilt root must equal
    /// `commitment.mmr_root`.
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

fn shape_at<B: StorageBackend + ?Sized>(backend: &B, hash: &H256) -> Result<NodeShape, Error> {
    backend.node_shape(hash)?.ok_or(Error::NodeNotFound(*hash))
}

/// Number of chunks under `root`, from the nodes down its right edge.
///
/// In the chunk tree the left child of a node spans a power of two of chunks,
/// so each node on the right edge adds the size of its left child, and the
/// last chunk adds one. An edge deeper than [`MAX_TREE_DEPTH`] is not a chunk
/// tree. Reads no chunk data.
fn chunk_count<B: StorageBackend + ?Sized>(backend: &B, root: H256) -> Result<u64, Error> {
    let mut count = 0u64;
    let mut hash = root;
    for _ in 0..=MAX_TREE_DEPTH {
        match shape_at(backend, &hash)? {
            NodeShape::Chunk { .. } => {
                return count
                    .checked_add(1)
                    .ok_or_else(|| Error::NonCanonicalTree(format!("{:#x}", root)))
            }
            NodeShape::Internal { left, right } => {
                count = count
                    .checked_add(left_child_chunks(backend, left, root)?)
                    .ok_or_else(|| Error::NonCanonicalTree(format!("{:#x}", root)))?;
                hash = right;
            }
        }
    }
    Err(Error::NonCanonicalTree(format!("{:#x}", root)))
}

/// Size of the first chunk under `root`, from the nodes down its left edge.
/// Reads no chunk data.
fn first_chunk_size<B: StorageBackend + ?Sized>(backend: &B, root: H256) -> Result<u64, Error> {
    let mut hash = root;
    for _ in 0..=MAX_TREE_DEPTH {
        match shape_at(backend, &hash)? {
            NodeShape::Chunk { size } => return Ok(size),
            NodeShape::Internal { left, .. } => hash = left,
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
        match shape_at(backend, &hash)? {
            NodeShape::Chunk { .. } => return Ok(1u64 << height),
            NodeShape::Internal { left, .. } => hash = left,
        }
    }
    Err(Error::NonCanonicalTree(format!("{:#x}", root)))
}

/// [`StorageBackend::read_chunks`] for a tree of `chunk_count` chunks. One
/// walk from the root visits each node on the paths to the read chunks once.
fn read_chunks_of<B: StorageBackend + ?Sized>(
    backend: &B,
    root: H256,
    chunk_count: u64,
    start: u64,
    max_chunks: u64,
    max_bytes: u64,
) -> Result<ChunkRead, Error> {
    let end = start.saturating_add(max_chunks).min(chunk_count);
    let mut walk = RangeWalk {
        backend,
        root,
        range: start..end,
        max_bytes,
        bytes: 0,
        siblings: Vec::new(),
        chunks: Vec::new(),
    };
    if start < end {
        walk.visit(root, chunk_count, 0)?;
    }
    Ok(ChunkRead {
        start_chunk: start,
        chunk_count,
        chunks: walk.chunks,
    })
}

/// State of [`read_chunks_of`]'s walk.
struct RangeWalk<'a, B: ?Sized> {
    backend: &'a B,
    root: H256,
    /// Chunk indices to read.
    range: core::ops::Range<u64>,
    max_bytes: u64,
    /// Chunk data bytes read so far.
    bytes: u64,
    /// Siblings on the path from the root to the current node.
    siblings: Vec<H256>,
    chunks: Vec<ProvenChunk>,
}

impl<B: StorageBackend + ?Sized> RangeWalk<'_, B> {
    /// Read the chunks in `self.range` under `hash`, the subtree over `count`
    /// chunks from index `first`. Returns `false` once `max_bytes` stops the
    /// read.
    fn visit(&mut self, hash: H256, count: u64, first: u64) -> Result<bool, Error> {
        if first >= self.range.end || first.saturating_add(count) <= self.range.start {
            return Ok(true);
        }
        match shape_at(self.backend, &hash)? {
            NodeShape::Chunk { size } if count == 1 => {
                let total = self.bytes.saturating_add(size);
                if !self.chunks.is_empty() && total > self.max_bytes {
                    return Ok(false);
                }
                self.bytes = total;
                let data = chunk_data(self.backend, self.root, &hash)?;
                let proof = self.siblings.iter().rev().copied().collect();
                self.chunks.push((hash, data, proof));
                Ok(true)
            }
            NodeShape::Internal { left, right } if count > 1 => {
                let split = 1u64 << (count - 1).ilog2();
                self.siblings.push(right);
                let more = self.visit(left, split, first)?;
                self.siblings.pop();
                if !more {
                    return Ok(false);
                }
                self.siblings.push(left);
                let more = self.visit(right, count - split, first + split)?;
                self.siblings.pop();
                Ok(more)
            }
            _ => Err(Error::NonCanonicalTree(format!("{:#x}", self.root))),
        }
    }
}

/// Hash and Merkle proof of chunk `index` in the tree of `chunk_count`
/// chunks under `root`. A tree over `n > 1` chunks splits into the largest
/// power of two below `n` on the left and the rest on the right, and the
/// proof lists the sibling at each split, from the chunk up. Reads no chunk
/// data.
fn locate_chunk<B: StorageBackend + ?Sized>(
    backend: &B,
    root: H256,
    chunk_count: u64,
    index: u64,
) -> Result<(H256, Vec<H256>), Error> {
    let (mut hash, mut count, mut index) = (root, chunk_count, index);
    let mut siblings = Vec::new();
    loop {
        match shape_at(backend, &hash)? {
            NodeShape::Chunk { .. } if count == 1 => {
                siblings.reverse();
                return Ok((hash, siblings));
            }
            NodeShape::Internal { left, right } if count > 1 => {
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

/// Data of the chunk `hash` in the tree under `root`.
fn chunk_data<B: StorageBackend + ?Sized>(
    backend: &B,
    root: H256,
    hash: &H256,
) -> Result<Vec<u8>, Error> {
    match node_at(backend, hash)? {
        ChunkTreeNode::Chunk(data) => Ok(data),
        ChunkTreeNode::Internal { .. } => Err(Error::NonCanonicalTree(format!("{:#x}", root))),
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

    /// A byte range maps to the chunks that cover it, with the chunk size of
    /// the root's first chunk.
    #[test]
    fn read_range_returns_the_chunks_that_cover_the_bytes() {
        // Five chunks of 32 bytes.
        let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &chunks(5));
        let root = build_merkle_tree(&storage, 1, &leaves).unwrap();

        let read = storage
            .read_range(root, 40, 30, u64::MAX, u64::MAX)
            .unwrap();
        assert_eq!((read.start_chunk, read.chunk_count), (1, 5));
        let hashes: Vec<H256> = read.chunks.iter().map(|c| c.0).collect();
        assert_eq!(hashes, leaves[1..3]);

        let read = storage.read_range(root, 0, u64::MAX, 2, u64::MAX).unwrap();
        assert_eq!(read.chunks.len(), 2, "max_chunks");
        let read = storage
            .read_range(root, u64::MAX, u64::MAX, 10, u64::MAX)
            .unwrap();
        assert!(read.chunks.is_empty());
    }

    /// A root of one chunk: a short chunk, and an empty one, are read whole
    /// from offset 0 and give nothing past their end.
    #[test]
    fn read_range_of_a_single_chunk_root() {
        let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &[vec![9u8; 100], vec![]]);
        let short = leaves[0];
        let read = storage.read_range(short, 0, 1 << 20, 10, u64::MAX).unwrap();
        assert_eq!((read.chunk_count, read.chunks.len()), (1, 1));
        assert!(storage
            .read_range(short, 200, 10, 10, u64::MAX)
            .unwrap()
            .chunks
            .is_empty());

        let empty = leaves[1];
        let read = storage.read_range(empty, 0, 1, 10, u64::MAX).unwrap();
        assert_eq!((read.chunk_count, read.chunks.len()), (1, 1));
        assert!(storage
            .read_range(empty, 0, 0, 10, u64::MAX)
            .unwrap()
            .chunks
            .is_empty());
    }

    /// A tree whose first chunk is empty but has more chunks is not a chunk
    /// tree: only the last chunk can be shorter.
    #[test]
    fn read_range_rejects_an_empty_first_chunk() {
        let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &[vec![], vec![1u8; 32]]);
        let root = build_merkle_tree(&storage, 1, &leaves).unwrap();
        assert!(matches!(
            storage.read_range(root, 0, 64, 10, u64::MAX),
            Err(Error::NonCanonicalTree(_))
        ));
    }

    /// `max_bytes` stops the read before the chunk that would pass it, but
    /// never returns zero chunks for a start inside the data. The chunk count
    /// is the count of the whole data root.
    #[test]
    fn read_chunks_stops_at_max_bytes_and_reports_the_chunk_count() {
        // Five chunks of 32 bytes.
        let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &chunks(5));
        let root = build_merkle_tree(&storage, 1, &leaves).unwrap();

        let read = storage.read_chunks(root, 1, 10, 64).unwrap();
        assert_eq!(read.chunk_count, 5);
        assert_eq!(read.chunks.len(), 2);
        assert_eq!(read.chunks[0].0, leaves[1]);

        let read = storage.read_chunks(root, 1, 10, 63).unwrap();
        assert_eq!(read.chunks.len(), 1, "one chunk even above max_bytes");

        let read = storage.read_chunks(root, 5, 10, u64::MAX).unwrap();
        assert_eq!(read.chunk_count, 5);
        assert!(read.chunks.is_empty());
    }

    #[test]
    fn read_chunks_match_single_chunks_and_clip_to_the_last_chunk() {
        let chunks = chunks(5);
        let (_dir, storage, leaves) = storage_with_chunks(1024 * 1024, &chunks);
        let root = build_merkle_tree(&storage, 1, &leaves).unwrap();

        let range = storage.read_chunks(root, 1, 3, u64::MAX).unwrap().chunks;
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
            .read_chunks(root, 3, 7, u64::MAX)
            .unwrap()
            .chunks
            .into_iter()
            .map(|(_, data, _)| data)
            .collect();
        assert_eq!(clipped, chunks[3..]);
        assert!(storage
            .read_chunks(root, 7, 2, u64::MAX)
            .unwrap()
            .chunks
            .is_empty());
        assert!(storage
            .read_chunks(root, 3, 0, u64::MAX)
            .unwrap()
            .chunks
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
                    .read_chunks(
                        root,
                        start as u64,
                        (end as u64).saturating_sub(start as u64),
                        u64::MAX,
                    )
                    .unwrap()
                    .chunks;
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
