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

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error(
        "Incompatible on-disk storage format at {path}: database is {found}, this build reads \
         and writes version {expected}. Migrate the database, or move it aside and re-sync this \
         provider's buckets from the client or a replica"
    )]
    IncompatibleFormat {
        /// Directory the database was opened from.
        path: String,
        /// What the database is marked with, in words.
        found: String,
        /// Version this build reads and writes.
        expected: u32,
    },
}
