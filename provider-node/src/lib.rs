//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)

pub mod api;
pub mod storage;
pub mod disk_storage;
pub mod mmr;
pub mod replica_sync;
pub mod types;
pub mod error;

pub use api::create_router;
pub use storage::Storage;
pub use disk_storage::DiskStorage;
pub use replica_sync::ReplicaSync;
pub use types::*;
pub use error::Error;

use std::sync::Arc;

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<Storage>,
    /// Provider account ID (hex encoded)
    pub provider_id: String,
}

impl ProviderState {
    pub fn new(storage: Arc<Storage>, provider_id: String) -> Self {
        Self {
            storage,
            provider_id,
        }
    }
}
