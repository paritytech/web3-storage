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

use sp_core::{sr25519, Pair, crypto::Ss58Codec};
use std::sync::Arc;

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<Storage>,
    /// Provider account ID (SS58 encoded)
    pub provider_id: String,
    /// Signing keypair (optional, for dev/testing)
    pub keypair: Option<sr25519::Pair>,
}

impl ProviderState {
    pub fn new(storage: Arc<Storage>, provider_id: String) -> Self {
        Self {
            storage,
            provider_id,
            keypair: None,
        }
    }

    /// Create with a seed phrase or derivation path (e.g., "//Alice", "//Bob").
    pub fn with_seed(storage: Arc<Storage>, seed: &str) -> Result<Self, String> {
        let keypair = sr25519::Pair::from_string(seed, None)
            .map_err(|e| format!("Failed to create keypair: {:?}", e))?;

        let provider_id = keypair.public().to_ss58check();

        Ok(Self {
            storage,
            provider_id,
            keypair: Some(keypair),
        })
    }

    /// Sign a message and return the signature as hex.
    pub fn sign(&self, message: &[u8]) -> String {
        match &self.keypair {
            Some(keypair) => {
                let signature = keypair.sign(message);
                format!("0x{}", hex::encode(signature.0))
            }
            None => {
                // Return placeholder if no keypair configured
                format!("0x{}", hex::encode([0u8; 64]))
            }
        }
    }
}
