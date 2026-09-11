// SPDX-License-Identifier: Apache-2.0

//! Error types for the storage engine.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<String>),

    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    #[error("Root not found: {0}")]
    RootNotFound(String),

    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    #[error("Serialization error: {0}")]
    Serialization(String),

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
}
