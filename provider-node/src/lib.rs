//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)
//! - Coordinating provider-initiated checkpoints

pub mod api;
pub mod challenge_responder;
pub mod checkpoint_coordinator;
pub mod cli;
pub mod command;
pub mod error;
pub mod mmr;
pub mod replica_sync;
pub mod replica_sync_coordinator;
pub mod storage;
pub mod types;

pub use api::create_router;
pub use challenge_responder::{
    ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle,
    ChallengeResponseResult, DetectedChallenge, ResponderCommand,
};
pub use checkpoint_coordinator::{
    CheckpointCoordinator, CheckpointCoordinatorConfig, CheckpointCoordinatorHandle,
    CheckpointDuty, CheckpointResult, CoordinatorCommand,
};
pub use error::Error;
pub use replica_sync::ReplicaSync;
pub use replica_sync_coordinator::{
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, ReplicaSyncCoordinatorHandle,
    SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use storage::{
    build_merkle_proof, hex_decode, hex_encode, BucketInfo, DiskStorage, Storage, StorageBackend,
    StoredNode,
};
pub use types::*;

use sp_core::{crypto::Ss58Codec, sr25519, Pair};
use std::sync::Arc;

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<dyn StorageBackend>,
    /// Provider account ID (SS58 encoded)
    pub provider_id: String,
    /// Signing keypair (optional, for dev/testing)
    pub keypair: Option<sr25519::Pair>,
}

impl ProviderState {
    pub fn new(storage: Arc<dyn StorageBackend>, provider_id: String) -> Self {
        Self {
            storage,
            provider_id,
            keypair: None,
        }
    }

    /// Create with a seed phrase or derivation path (e.g., "//Alice", "//Bob").
    pub fn with_seed(storage: Arc<dyn StorageBackend>, seed: &str) -> Result<Self, String> {
        let keypair = sr25519::Pair::from_string(seed, None)
            .map_err(|e| format!("Failed to create keypair: {e:?}"))?;

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
