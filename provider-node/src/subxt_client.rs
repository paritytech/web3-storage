// SPDX-License-Identifier: GPL-3.0-only

//! Thin adapters implementing the coordinators' chain-client traits for the
//! shared [`SubxtChainClient`] from `provider-subxt-client`.
//!
//! The client crate exposes inherent methods returning its own error type;
//! the impls here delegate 1:1 and convert errors, so each coordinator keeps
//! depending only on the narrow trait it needs (and per-trait mocks keep
//! working).

use crate::challenge_responder::{ChallengeChainClient, DetectedChallenge};
use crate::replica_sync_coordinator::{
    BucketSnapshot, ReplicaAgreementInfo, ReplicaSyncChainClient,
};
use crate::Error;
use sp_core::H256;
use storage_primitives::BucketId;

pub use provider_subxt_client::{fetch_current_anchor_block, SubxtChainClient};

#[async_trait::async_trait]
impl ReplicaSyncChainClient for SubxtChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        Ok(self.get_current_block().await?)
    }

    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        Ok(self
            .fetch_replica_agreements(provider_account, local_buckets)
            .await?)
    }

    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        Ok(self.fetch_bucket_snapshot(bucket_id).await?)
    }

    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        Ok(self.fetch_primary_endpoints(bucket_id).await?)
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        Ok(self
            .submit_sync_confirmation(bucket_id, target_mmr_root)
            .await?)
    }
}

#[async_trait::async_trait]
impl ChallengeChainClient for SubxtChainClient {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        Ok(self.poll_challenges().await?)
    }

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error> {
        Ok(self.fetch_challenge(deadline, index).await?)
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        Ok(self
            .submit_response(challenge_id, chunk_data, mmr_proof, chunk_proof)
            .await?)
    }
}
