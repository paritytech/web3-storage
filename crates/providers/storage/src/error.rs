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

    /// Storage exceeds allowance (507)
    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    /// Cannot find bucket by given id (400)
    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    /// Cannot find MMR root (400)
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
}
