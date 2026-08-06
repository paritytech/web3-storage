// SPDX-License-Identifier: Apache-2.0

//! Replica synchronization for provider nodes: the HTTP protocol replicas use
//! to pull data from primaries, and the background coordinator that drives it.

pub mod coordinator;
pub mod sync;

pub use coordinator::{
    ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use sync::ReplicaSync;

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

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Map storage-engine errors onto this crate's error space one-to-one, the
/// same mapping provider-node's own `Error` uses.
impl From<provider_storage::Error> for Error {
    fn from(e: provider_storage::Error) -> Self {
        use provider_storage::Error as StorageError;
        match e {
            StorageError::NodeNotFound(hash) => Error::NodeNotFound(hash),
            StorageError::ChildrenMissing(children) => Error::ChildrenMissing(children),
            StorageError::QuotaExceeded { used, max } => Error::QuotaExceeded { used, max },
            StorageError::BucketNotFound(id) => Error::BucketNotFound(id),
            StorageError::RootNotFound(root) => Error::RootNotFound(root),
            StorageError::InvalidHash { expected, actual } => {
                Error::InvalidHash { expected, actual }
            }
            StorageError::Storage(msg) => Error::Storage(msg),
            StorageError::Serialization(msg) => Error::Serialization(msg),
        }
    }
}
