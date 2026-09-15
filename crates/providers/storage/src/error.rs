// SPDX-License-Identifier: Apache-2.0

//! Error types for the storage engine.

use sp_core::H256;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0:?}")]
    NodeNotFound(H256),

    /// A chunk, leaf, or proof addressed by index rather than by hash.
    #[error("Resource not found: {0}")]
    ResourceNotFound(String),

    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<H256>),

    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    #[error("Root not found: {0:?}")]
    RootNotFound(H256),

    #[error("Invalid hash: expected {expected:?}, got {actual:?}")]
    InvalidHash { expected: H256, actual: H256 },

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
