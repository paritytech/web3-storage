//! Challenger Client - For third parties verifying data integrity.
//!
//! This client provides operations for:
//! - Monitoring provider performance
//! - Creating challenges to verify data availability
//! - Collecting rewards from successful challenges
//! - Automated challenge strategies

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::substrate::{extrinsics, SubstrateClient};
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::BucketId;

/// Client for challengers (third parties who verify data integrity).
pub struct ChallengerClient {
    base: BaseClient,
    challenger_account: String, // Substrate account ID
}

impl ChallengerClient {
    /// Create a new challenger client.
    pub fn new(config: ClientConfig, challenger_account: String) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            challenger_account,
        })
    }

    /// Create with default configuration.
    pub fn with_defaults(challenger_account: String) -> ClientResult<Self> {
        Self::new(ClientConfig::default(), challenger_account)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Challenge Operations
    // ═════════════════════════════════════════════════════════════════════════

    /// Challenge a provider based on the on-chain checkpoint.
    ///
    /// This is the safest challenge mode - uses the canonical snapshot
    /// that's already on-chain. No off-chain signature needed.
    ///
    /// # Parameters
    /// - `bucket_id`: Bucket to challenge
    /// - `provider`: Provider to challenge
    /// - `leaf_index`: Which leaf in the MMR to challenge
    /// - `chunk_index`: Which chunk within that leaf to challenge
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::ChallengerClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ChallengerClient::with_defaults("5GrwvaEF...".to_string())?;
    ///
    /// // Challenge a random chunk
    /// let bucket_id = 1;
    /// let provider = "5FHneW46...".to_string();
    /// let leaf_index = 5;
    /// let chunk_index = 123;
    ///
    /// client.challenge_checkpoint(bucket_id, provider, leaf_index, chunk_index).await?;
    /// println!("Challenge created!");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn challenge_checkpoint(
        &self,
        bucket_id: BucketId,
        provider: String,
        leaf_index: u64,
        chunk_index: u64,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Challenging {} on bucket {} checkpoint (leaf {}, chunk {})",
            provider,
            bucket_id,
            leaf_index,
            chunk_index
        );

        // Parse provider account
        let provider_account = SubstrateClient::parse_account(&provider)?;

        // Create and submit the extrinsic
        let tx = extrinsics::challenge_checkpoint(bucket_id, provider_account, leaf_index, chunk_index);

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {}", e)))?;

        // Wait for finalization and extract challenge ID from events
        let events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {}", e)))?;

        tracing::info!("Challenge created successfully");

        // TODO: Extract actual challenge ID from events
        Ok(ChallengeId {
            deadline: 1000,
            index: 0,
        })
    }

    /// Challenge a provider using their off-chain commitment signature.
    ///
    /// Use this for "hot" buckets where the checkpoint changes frequently.
    /// You need to have obtained a signed commitment from the provider off-chain.
    ///
    /// # Parameters
    /// - `provider_signature`: The provider's signature on the commitment
    pub async fn challenge_offchain(
        &self,
        bucket_id: BucketId,
        provider: String,
        mmr_root: H256,
        start_seq: u64,
        leaf_index: u64,
        chunk_index: u64,
        provider_signature: Vec<u8>,
    ) -> ClientResult<ChallengeId> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would challenge {} on bucket {} using off-chain commitment",
            provider,
            bucket_id
        );

        Ok(ChallengeId {
            deadline: 1000,
            index: 0,
        })
    }

    /// Challenge a replica provider based on their sync confirmation.
    ///
    /// Replicas confirm syncs on-chain, so you can challenge based on their
    /// last_sync stored in the agreement.
    pub async fn challenge_replica(
        &self,
        bucket_id: BucketId,
        provider: String,
        leaf_index: u64,
        chunk_index: u64,
    ) -> ClientResult<ChallengeId> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would challenge replica {} on bucket {}",
            provider,
            bucket_id
        );

        Ok(ChallengeId {
            deadline: 1000,
            index: 0,
        })
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring & Strategy
    // ═════════════════════════════════════════════════════════════════════════

    /// Get all active challenges you've created.
    pub async fn list_my_challenges(&self) -> ClientResult<Vec<ChallengeInfo>> {
        // TODO: Query chain storage
        Ok(vec![])
    }

    /// Monitor a provider for challengeable behavior.
    ///
    /// Returns recommendations on whether to challenge.
    pub async fn analyze_provider(
        &self,
        bucket_id: BucketId,
        provider: String,
    ) -> ClientResult<ProviderAnalysis> {
        // TODO: Fetch provider stats, commitment freshness, etc.
        Ok(ProviderAnalysis {
            provider,
            reputation: 85,
            last_checkpoint_age: 100,
            challenge_success_rate: 95.0,
            recommendation: ChallengeRecommendation::Monitor,
        })
    }

    /// Automated challenge strategy: randomly challenge providers with low reputation.
    ///
    /// This can run in a loop to automatically earn challenge rewards.
    pub async fn auto_challenge_strategy(
        &self,
        min_reputation_threshold: u8,
        max_challenges_per_round: usize,
    ) -> ClientResult<Vec<ChallengeId>> {
        tracing::info!(
            "Running auto-challenge strategy: min_reputation={}, max_challenges={}",
            min_reputation_threshold,
            max_challenges_per_round
        );

        // TODO: Implement strategy
        // 1. Query all providers
        // 2. Filter by reputation < threshold
        // 3. For each provider, analyze their buckets
        // 4. Create challenges for suspicious ones
        // 5. Return challenge IDs

        Ok(vec![])
    }

    /// Check if a challenge has been resolved and claim rewards if successful.
    pub async fn check_and_claim_reward(
        &self,
        challenge_id: ChallengeId,
    ) -> ClientResult<Option<u128>> {
        // TODO: Query challenge status
        // If provider failed to respond, rewards were already distributed in on_finalize
        // If provider responded, check the cost split

        Ok(None)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Analytics
    // ═════════════════════════════════════════════════════════════════════════

    /// Get your total earnings from challenges.
    pub async fn get_total_challenge_earnings(&self) -> ClientResult<u128> {
        // TODO: Calculate from challenge history
        Ok(0)
    }

    /// Get statistics about your challenge activity.
    pub async fn get_challenge_stats(&self) -> ClientResult<ChallengeStats> {
        // TODO: Query chain data
        Ok(ChallengeStats {
            total_challenges: 0,
            successful_challenges: 0,
            failed_challenges: 0,
            total_earnings: 0,
            avg_response_time: 0,
        })
    }

    /// Find the most profitable providers to challenge.
    ///
    /// Returns providers ranked by potential reward vs risk.
    pub async fn find_challenge_targets(&self, limit: usize) -> ClientResult<Vec<ChallengeTarget>> {
        // TODO: Analyze on-chain data
        // - Providers with low reputation
        // - Providers with high stakes (higher rewards if they fail)
        // - Providers with stale checkpoints
        // - Providers in buckets with valuable data

        Ok(vec![])
    }
}

// Types

#[derive(Debug, Clone, Copy)]
pub struct ChallengeId {
    pub deadline: u32,
    pub index: u16,
}

#[derive(Debug, Clone)]
pub struct ChallengeInfo {
    pub challenge_id: ChallengeId,
    pub bucket_id: BucketId,
    pub provider: String,
    pub deadline: u32,
    pub deposit: u128,
    pub status: ChallengeStatus,
}

#[derive(Debug, Clone)]
pub enum ChallengeStatus {
    Pending,
    Responded { response_time: u32 },
    Expired,
}

#[derive(Debug, Clone)]
pub struct ProviderAnalysis {
    pub provider: String,
    pub reputation: u8,
    pub last_checkpoint_age: u32,
    pub challenge_success_rate: f64,
    pub recommendation: ChallengeRecommendation,
}

#[derive(Debug, Clone)]
pub enum ChallengeRecommendation {
    /// Highly recommended to challenge
    Challenge,
    /// Monitor but don't challenge yet
    Monitor,
    /// Don't challenge (provider is reliable)
    Skip,
}

#[derive(Debug, Clone, Default)]
pub struct ChallengeStats {
    pub total_challenges: u32,
    pub successful_challenges: u32,
    pub failed_challenges: u32,
    pub total_earnings: u128,
    pub avg_response_time: u32,
}

#[derive(Debug, Clone)]
pub struct ChallengeTarget {
    pub provider: String,
    pub bucket_id: BucketId,
    pub potential_reward: u128,
    pub success_probability: f64,
    pub expected_value: u128,
}
