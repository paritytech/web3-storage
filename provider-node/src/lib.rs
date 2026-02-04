//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)

pub mod api;
pub mod disk_storage;
pub mod error;
pub mod mmr;
pub mod replica_sync;
pub mod storage;
pub mod types;

pub use api::create_router;
pub use disk_storage::DiskStorage;
pub use error::Error;
pub use replica_sync::ReplicaSync;
pub use storage::Storage;
pub use types::*;

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
