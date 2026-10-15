// SPDX-License-Identifier: Apache-2.0

//! Error types for the storage engine.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// Cannot find node (404)
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    /// Missing children (400)
    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<String>),

    /// An internal node arrived with a child list that is not exactly two
    /// hashes. Send the left and right child hashes.
    #[error("Invalid child count: expected 2, got {0}")]
    InvalidChildCount(usize),

    /// An internal node arrived with `data` that is not the left child hash
    /// followed by the right child hash. Send exactly those 64 bytes.
    #[error("Internal node data is not the concatenation of its children")]
    InvalidInternalNodeData,

    /// Storage exceeds allowance (507)
    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    /// A leaf above the size a challenge response can contain. The provider
    /// cannot prove it on-chain, so it rejects it at store and at commit.
    #[error("Chunk too large: {size} bytes, max {max}")]
    ChunkTooLarge { size: u64, max: u32 },

    /// Cannot find bucket by given id (404)
    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    /// Cannot find MMR root (404)
    #[error("Root not found: {0}")]
    RootNotFound(String),

    /// Invalid leaf hash (400)
    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    /// Invalid tree shape (400)
    #[error("Non-canonical tree: {0}")]
    NonCanonicalTree(String),

    /// Serialization error (400)
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Internal RocksDb errors (500)
    #[error("RocksDB error: {0}")]
    RocksDb(#[from] rocksdb::Error),

    /// A column family the engine expects was absent (500)
    #[error("Column family not found: {0}")]
    ColumnFamilyMissing(&'static str),

    /// The database on disk was written in a format this build cannot read.
    /// Opening it anyway would decode its records wrongly, so the engine
    /// refuses rather than reporting phantom missing data.
    #[error("Incompatible on-disk format: {0}")]
    IncompatibleFormat(String),
}
