// SPDX-License-Identifier: Apache-2.0

//! Subxt-based production chain client shared by storage provider node
//! coordinators.
//!
//! Extracted from the provider node so the chain-facing plumbing (connection
//! construction, signing client) can be reused without depending on the
//! node's internal state.

pub mod chain_connection;
pub mod subxt_client;

pub use subxt_client::{fetch_current_anchor_block, SubxtChainClient};

use sp_core::H256;
use std::sync::Arc;
use storage_primitives::BucketId;

/// Errors produced by the chain client.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Information about a detected challenge.
#[derive(Clone, Debug)]
pub struct DetectedChallenge {
    /// Bucket being challenged.
    pub bucket_id: BucketId,
    /// Challenge deadline (block number).
    pub deadline: u32,
    /// Challenge index within the deadline.
    pub index: u16,
    /// MMR root being challenged.
    pub mmr_root: H256,
    /// Start sequence of the commitment.
    pub start_seq: u64,
    /// Leaf index in the MMR to prove.
    pub leaf_index: u64,
    /// Chunk index within the leaf to prove.
    pub chunk_index: u64,
    /// Challenger's account.
    pub challenger: String,
}

/// Information about a replica agreement from chain.
#[derive(Clone, Debug)]
pub struct ReplicaAgreementInfo {
    pub bucket_id: BucketId,
    pub sync_balance: u128,
    pub sync_price: u128,
    pub min_sync_interval: u64,
    pub last_sync: Option<(H256, u64)>,
}

/// Bucket snapshot from chain.
#[derive(Clone, Debug)]
pub struct BucketSnapshot {
    pub mmr_root: H256,
    pub leaf_count: u64,
}

/// Trait abstracting chain interactions for the challenge responder.
#[async_trait::async_trait]
pub trait ChallengeChainClient: Send + Sync {
    /// Poll the chain for active challenges targeting this provider.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error>;

    /// Point-read a single challenge by id, `None` if it is gone (already
    /// responded / reaped) or targets another provider. Backs the
    /// event-driven path, where `ChallengeCreated` carries the id but not
    /// the proof parameters.
    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error>;

    /// Submit a challenge response transaction.
    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error>;
}

#[async_trait::async_trait]
impl<T: ChallengeChainClient> ChallengeChainClient for Arc<T> {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        self.as_ref().poll_challenges().await
    }

    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error> {
        self.as_ref().fetch_challenge(deadline, index).await
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        self.as_ref()
            .submit_response(challenge_id, chunk_data, mmr_proof, chunk_proof)
            .await
    }
}

/// Trait abstracting chain interactions for the replica sync coordinator.
#[async_trait::async_trait]
pub trait ReplicaSyncChainClient: Send + Sync {
    /// Get the current block number.
    async fn get_current_block(&self) -> Result<u64, Error>;

    /// Fetch replica agreements for this provider.
    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error>;

    /// Fetch the bucket snapshot (latest checkpoint state) from chain.
    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error>;

    /// Fetch primary provider HTTP endpoints for a bucket.
    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error>;

    /// Submit a confirm_replica_sync extrinsic.
    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error>;
}

#[async_trait::async_trait]
impl<T: ReplicaSyncChainClient> ReplicaSyncChainClient for Arc<T> {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.as_ref().get_current_block().await
    }

    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        self.as_ref()
            .fetch_replica_agreements(provider_account, local_buckets)
            .await
    }

    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        self.as_ref().fetch_bucket_snapshot(bucket_id).await
    }

    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        self.as_ref().fetch_primary_endpoints(bucket_id).await
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        self.as_ref()
            .submit_sync_confirmation(bucket_id, target_mmr_root)
            .await
    }
}

/// Manually-decoded view of a `Challenge` struct from raw SCALE bytes.
///
/// We avoid the `subxt::dynamic::Value` -> typed conversion because that
/// requires metadata-aware decoding of generic `BalanceOf<T>` etc. The byte
/// layout of `Challenge<T>` is stable for the deployed runtimes, so we read
/// fixed offsets.
///
/// Exposed (`#[doc(hidden)]`) only so the fixed-offset layout can be exercised
/// from an integration test against the encoded `Challenge<T>` bytes — it is
/// not part of the crate's stable public API.
#[doc(hidden)]
pub struct DecodedChallenge {
    pub bucket_id: u64,
    pub challenger: [u8; 32],
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_index: u64,
    pub chunk_index: u64,
}

/// Total SCALE-encoded size of a single `Challenge<T>` value (fixed-width
/// fields only, see the layout below).
const CHALLENGE_ENTRY_SIZE: usize = 144;

/// Decode a single SCALE-encoded `Challenge` value from `Challenges` storage
/// (the map is now a `StorageDoubleMap<BlockNumber, u16, Challenge>`, so each
/// key holds exactly one challenge rather than a `Vec`). Returns `Some` iff
/// the decoded `provider` field matches `our_bytes`; `None` when the
/// challenge targets a different provider.
///
/// Layout of `Challenge<T>` (see `crates/pallets/storage-provider/src/lib.rs`):
///   bucket_id (u64)         — 8
///   provider (AccountId32)  — 32
///   challenger (AccountId32)— 32
///   mmr_root (H256)         — 32
///   start_seq (u64)         — 8
///   leaf_index (u64)        — 8
///   chunk_index (u64)       — 8
///   deposit (Balance u128)  — 16
/// Total: 144 bytes.
///
/// `#[doc(hidden)] pub` so the fixed-offset layout is reachable from an
/// integration test; it is an internal helper, not stable public API.
#[doc(hidden)]
pub fn decode_challenge_for_provider(
    encoded: &[u8],
    our_bytes: &[u8; 32],
) -> Result<Option<DecodedChallenge>, &'static str> {
    if encoded.len() < CHALLENGE_ENTRY_SIZE {
        return Err("challenge value shorter than expected layout");
    }
    let entry = &encoded[..CHALLENGE_ENTRY_SIZE];

    let provider = &entry[8..40];
    if provider != our_bytes {
        return Ok(None);
    }

    let bucket_id = u64::from_le_bytes(entry[0..8].try_into().expect("8 bytes"));
    let mut challenger = [0u8; 32];
    challenger.copy_from_slice(&entry[40..72]);
    let mut root_bytes = [0u8; 32];
    root_bytes.copy_from_slice(&entry[72..104]);
    let mmr_root = H256::from(root_bytes);
    let start_seq = u64::from_le_bytes(entry[104..112].try_into().expect("8 bytes"));
    let leaf_index = u64::from_le_bytes(entry[112..120].try_into().expect("8 bytes"));
    let chunk_index = u64::from_le_bytes(entry[120..128].try_into().expect("8 bytes"));
    // deposit at entry[128..144] — not needed for the response.

    Ok(Some(DecodedChallenge {
        bucket_id,
        challenger,
        mmr_root,
        start_seq,
        leaf_index,
        chunk_index,
    }))
}
