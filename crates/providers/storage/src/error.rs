// SPDX-License-Identifier: Apache-2.0

//! Error types for the storage engine.

use sp_core::H256;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0:?}")]
    NodeNotFound(H256),

    /// A chunk index past the end of a data root's chunk list.
    #[error("Chunk not found: index {chunk_index} in data root {data_root:?}")]
    ChunkNotFound { data_root: H256, chunk_index: u64 },

    /// A leaf index past the end of a bucket's leaf list.
    #[error("Leaf not found: index {leaf_index} in bucket {bucket_id}")]
    LeafNotFound { bucket_id: u64, leaf_index: u64 },

    /// The MMR holds no proof for this leaf index.
    #[error("MMR proof not found: leaf index {leaf_index} in bucket {bucket_id}")]
    MmrProofNotFound { bucket_id: u64, leaf_index: u64 },

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
