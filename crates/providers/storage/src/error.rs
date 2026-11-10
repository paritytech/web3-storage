// SPDX-License-Identifier: Apache-2.0

//! Error types for the storage engine.

use sp_core::H256;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0:?}")]
    NodeNotFound(H256),

    /// A leaf index past the end of a bucket's leaf list.
    #[error("Leaf not found: index {leaf_index} in bucket {bucket_id}")]
    LeafNotFound { bucket_id: u64, leaf_index: u64 },

    #[error("Commitment not reproducible from local leaves: {0}")]
    CommitmentNotReproducible(String),

    /// A commitment cites leaves this store does not hold: they were pruned
    /// below the local `start_seq`, or lie past the local end.
    #[error(
        "Leaves {start_seq}+{leaf_count} not held locally (local {local_start_seq}..{local_end})"
    )]
    LeavesNotHeld {
        start_seq: u64,
        leaf_count: u64,
        local_start_seq: u64,
        local_end: u64,
    },

    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<H256>),

    /// An internal node arrived with a child list that is not exactly two
    /// hashes. Send the left and right child hashes.
    #[error("Invalid child count: expected 2, got {0}")]
    InvalidChildCount(usize),

    /// A node arrived with both `data` and `children`, or with neither. Send
    /// `data` for a chunk or `children` for an internal node.
    #[error("A node is either data (a chunk) or children (an internal node)")]
    InvalidNode,

    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    #[error("Invalid start_seq: requested {requested}, valid range {current}..={end}")]
    InvalidStartSeq {
        requested: u64,
        current: u64,
        end: u64,
    },

    /// A leaf chunk above `DEFAULT_CHUNK_SIZE` bytes. Split the data into chunks
    /// of at most that size.
    #[error("Chunk too large: {size} bytes, max {max}", max = storage_primitives::DEFAULT_CHUNK_SIZE)]
    ChunkTooLarge { size: u64 },

    /// `chunk_index` is at or beyond the chunk count of the tree under the
    /// data root: the leaf has no such chunk.
    #[error("Chunk index {chunk_index} out of range: the tree has {chunk_count} chunks")]
    ChunkIndexOutOfRange { chunk_index: u64, chunk_count: u64 },

    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    /// The bucket is being torn down on this provider: its agreement ended or
    /// the bucket was deleted. The provider accepts no more nodes or commits
    /// for it. Upload to a provider that holds an agreement for the bucket.
    #[error("Bucket {0} is condemned")]
    BucketCondemned(u64),

    /// The deletion receipt's signature is not a valid signature by its
    /// `admin` over the deletion commitment of the bucket. The admin must sign
    /// `CommitmentPayload(bucket_id, Commitment { mmr_root, start_seq:
    /// new_start_seq, leaf_count: 0 })`.
    #[error("Invalid deletion receipt for bucket {0}")]
    InvalidDeletionReceipt(u64),

    #[error("Root not found: {0:?}")]
    RootNotFound(H256),

    /// Another writer held the bucket past the lock timeout. Retry the
    /// request.
    #[error("Bucket {0} is busy")]
    BucketBusy(u64),

    /// The content tree under a data root has more than `max_nodes` nodes
    /// when every shared subtree is counted once per path that reaches it.
    /// Deduplication lets a few stored nodes span such a tree.
    #[error("Content tree exceeds {max_nodes} nodes")]
    TreeTooLarge { max_nodes: u64 },

    #[error("Invalid hash: expected {expected:?}, got {actual:?}")]
    InvalidHash { expected: H256, actual: H256 },

    /// The tree under a data root is not `merkle_tree` over chunks of
    /// `DEFAULT_CHUNK_SIZE` bytes, with only the last chunk shorter. Split the
    /// data into such chunks and upload the nodes of
    /// `storage_primitives::merkle_tree`.
    #[error("Non-canonical tree: {0}")]
    NonCanonicalTree(String),

    /// A stored record could not be decoded.
    #[error("Serialization error: {0}")]
    Serialization(#[from] codec::Error),

    /// Building the bucket MMR or a proof from it failed.
    #[error("MMR error: {0}")]
    Mmr(#[from] storage_primitives::mmr::Error),

    /// The RocksDB engine itself failed. Carries the engine's error as
    /// `source()` so the cause survives instead of being flattened into a
    /// message.
    #[error("RocksDB error: {0}")]
    RocksDb(#[from] rocksdb::Error),

    /// A column family the engine expects was absent from the open database:
    /// a layout bug or a database written by a different build, not an I/O
    /// fault.
    #[error("Column family not found: {0}")]
    ColumnFamilyMissing(&'static str),

    /// The database on disk was written in a format this build cannot read.
    /// Opening it anyway would decode its records wrongly, so the engine
    /// refuses rather than reporting phantom missing data.
    #[error("Incompatible on-disk format: {0}")]
    IncompatibleFormat(String),
}
