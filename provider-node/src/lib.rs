// SPDX-License-Identifier: GPL-3.0-only

//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)

pub mod chain_connection;
pub(crate) mod chain_follower;
pub mod challenge_proofs;
pub mod cli;
pub mod command;
pub(crate) mod event_decoding;
pub mod membership;
pub(crate) mod subxt_client;

pub use challenge_proofs::StorageProofSource;
pub use provider_challenge::{
    self as challenge_responder, ChallengeChainClient, ChallengeError, ChallengeProofSource,
    ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle,
    ChallengeResponseResult, DetectedChallenge, ResponderCommand,
};
/// The chain-state coordinator lives in the `provider-coordinator` crate; keep
/// the old module path working for existing consumers.
pub use provider_coordinator as chain_state_coordinator;
pub use provider_coordinator::{
    is_relevant_provider_event, refresh_if_relevant_event, refresh_provider_state, sync_constants,
    ChainFollower, ChainState, ChainStateChainClient, ChainStateCoordinator,
    ChainStateCoordinatorHandle, NonceCounter, PalletConstants, ProviderLifecycleEvent,
};
pub use provider_replica::{
    ReplicaSync, ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, SignedSyncRoots, SyncCommand, SyncCoordinatorStatus, SyncDuty,
    SyncResult, SyncRoots, SyncRootsSigner,
};
