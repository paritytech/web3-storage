// SPDX-License-Identifier: Apache-2.0

//! Checkpoint-driven coordinator services for storage provider nodes.
//!
//! Extracted from the provider node so the coordination logic depends only on
//! narrow traits ([`ReplicaStore`], [`ReplicaSyncChainClient`]) instead of the
//! node's internal state.

pub mod replica_sync_coordinator;

pub use replica_sync_coordinator::{
    BucketSnapshot, ReplicaAgreementInfo, ReplicaSyncChainClient, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, ReplicaSyncCoordinatorHandle, SyncCommand, SyncCoordinatorStatus,
    SyncDuty, SyncResult,
};

use sp_core::H256;
use storage_primitives::BucketId;

/// Errors produced by the coordinators in this crate.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Chain error: {0}")]
    Chain(String),

    #[error("Sync error: {0}")]
    Sync(String),
}

/// Local replica state and sync capability driven by the coordinator.
///
/// Implemented by the provider node over its storage backend and sync engine.
#[async_trait::async_trait]
pub trait ReplicaStore: Send + Sync {
    /// SS58 account of this provider.
    fn provider_id(&self) -> String;

    /// Bucket ids present in local storage.
    async fn local_bucket_ids(&self) -> Vec<BucketId>;

    /// Current local MMR root for a bucket, if the bucket exists locally.
    async fn local_mmr_root(&self, bucket_id: BucketId) -> Option<H256>;

    /// Sync a bucket from a primary provider; returns the synced MMR root.
    async fn sync_from_primary(
        &self,
        bucket_id: BucketId,
        primary_url: &str,
    ) -> Result<H256, Error>;
}
