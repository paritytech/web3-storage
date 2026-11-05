// SPDX-License-Identifier: Apache-2.0

//! Error types for the storage engine.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Commitment not reproducible from local leaves: {0}")]
    CommitmentNotReproducible(String),

    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<String>),

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

    #[error("Root not found: {0}")]
    RootNotFound(String),

    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    /// The tree under a data root is not `merkle_tree` over chunks of
    /// `DEFAULT_CHUNK_SIZE` bytes, with only the last chunk shorter. Split the
    /// data into such chunks and upload the nodes of
    /// `storage_primitives::merkle_tree`.
    #[error("Non-canonical tree: {0}")]
    NonCanonicalTree(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

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
