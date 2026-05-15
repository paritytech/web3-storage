//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)
//! - Coordinating provider-initiated checkpoints

pub mod agreement_coordinator;
pub mod api;
pub mod auth;
pub mod chain_client;
pub mod chain_stream;
pub mod challenge_responder;
pub mod checkpoint_coordinator;
pub mod cli;
pub mod command;
pub mod error;
pub mod fs_api;
pub mod fs_index;
pub mod mmr;
pub mod replica_sync;
pub mod replica_sync_coordinator;
pub mod s3_api;
pub mod s3_index;
pub mod storage;
pub mod types;

pub use agreement_coordinator::{
    AgreementCoordinator, AgreementCoordinatorConfig, AgreementCoordinatorHandle,
};
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
pub use fs_index::FsIndexManager;
pub use replica_sync::ReplicaSync;
pub use replica_sync_coordinator::{
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, ReplicaSyncCoordinatorHandle,
    SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use s3_index::S3IndexManager;
pub use storage::{
    build_merkle_proof, build_padded_merkle_tree, hex_decode, hex_encode, BucketInfo, DiskStorage,
    Storage, StorageBackend, StoredNode,
};
pub use types::*;

use sp_core::{crypto::Ss58Codec, sr25519, Pair};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<dyn StorageBackend>,
    /// Provider account ID (SS58 encoded)
    pub provider_id: String,
    /// Signing keypair (optional, for dev/testing)
    pub keypair: Option<sr25519::Pair>,
    /// S3-compatible object index
    pub s3_index: S3IndexManager,
    /// File system drive index
    pub fs_index: FsIndexManager,
    /// Channel to send commands to the checkpoint coordinator (if running).
    pub checkpoint_cmd_tx: std::sync::Mutex<Option<mpsc::Sender<CoordinatorCommand>>>,
    /// Whether auth is enabled (opt-in).
    pub auth_enabled: bool,
    /// Membership cache for role lookups (only set when auth is enabled).
    pub membership_cache: Option<Arc<auth::MembershipCache>>,
    /// Maximum allowed clock skew for request timestamps.
    pub auth_max_skew: Duration,
}

impl ProviderState {
    pub fn new(storage: Arc<dyn StorageBackend>, provider_id: String) -> Self {
        Self {
            storage,
            provider_id,
            keypair: None,
            s3_index: S3IndexManager::new(),
            fs_index: FsIndexManager::new(),
            checkpoint_cmd_tx: std::sync::Mutex::new(None),
            auth_enabled: false,
            membership_cache: None,
            auth_max_skew: Duration::from_secs(300),
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
            s3_index: S3IndexManager::new(),
            fs_index: FsIndexManager::new(),
            checkpoint_cmd_tx: std::sync::Mutex::new(None),
            auth_enabled: false,
            membership_cache: None,
            auth_max_skew: Duration::from_secs(300),
        })
    }

    /// Set the checkpoint coordinator command sender (called after coordinator starts).
    pub fn set_checkpoint_handle(&self, handle: &CheckpointCoordinatorHandle) {
        if let Ok(mut tx) = self.checkpoint_cmd_tx.lock() {
            *tx = Some(handle.command_sender());
        }
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
