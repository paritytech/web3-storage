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

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}
