// SPDX-License-Identifier: Apache-2.0

//! Challenger Client - For third parties verifying data integrity.
//!
//! This client provides operations for:
//! - Monitoring provider performance
//! - Creating challenges to verify data availability
//! - Collecting rewards from successful challenges
//! - Automated challenge strategies

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::substrate::{extrinsics, storage, SubstrateClient};
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::storage_primitives::ChallengerStatRecord;
use storage_subxt::api::storage_provider::events::ChallengeCreated as EvChallengeCreated;
use storage_subxt::subxt::blocks::ExtrinsicEvents;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt::utils::H256;
use storage_subxt::subxt::PolkadotConfig;

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

    /// Connect to the blockchain. Must be called before any on-chain operations.
    pub async fn connect(&mut self) -> ClientResult<()> {
        self.base.connect_chain().await
    }

    /// Set a development signer (alice, bob, charlie, dave, eve, ferdie).
    /// Must be called after connect().
    pub fn set_dev_signer(&mut self, name: &str) -> ClientResult<()> {
        self.base.set_dev_signer(name)
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
        let tx =
            extrinsics::challenge_checkpoint(bucket_id, provider_account, leaf_index, chunk_index);

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        // Wait for finalization and extract challenge ID from events
        let events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        let challenge_id = Self::extract_challenge_id(&events)?;
        tracing::info!(
            "Challenge created: deadline={}, index={}",
            challenge_id.deadline,
            challenge_id.index
        );

        Ok(challenge_id)
    }

    /// Challenge a provider using their off-chain commitment signature.
    ///
    /// Use this for "hot" buckets where the checkpoint changes frequently.
    /// You need to have obtained a signed commitment from the provider off-chain.
    ///
    /// # Parameters
    /// - `mmr_root`: The MMR root from the provider's commitment
    /// - `start_seq`: The start sequence from the commitment
    /// - `provider_signature`: The provider's signature on the commitment (64 bytes for Sr25519)
    #[allow(clippy::too_many_arguments)]
    pub async fn challenge_offchain(
        &self,
        bucket_id: BucketId,
        provider: String,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        leaf_index: u64,
        chunk_index: u64,
        nonce: u64,
        provider_signature: Vec<u8>,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::debug!(
            "Challenging {} on bucket {} using off-chain commitment (total leave {}, leaf index {}, chunk index {}) with nonce {}",
            provider,
            bucket_id,
            leaf_count,
            leaf_index,
            chunk_index,
            nonce
        );

        // Parse provider account
        let provider_account = SubstrateClient::parse_account(&provider)?;

        // Create and submit the extrinsic
        let tx = extrinsics::challenge_offchain(
            bucket_id,
            provider_account,
            mmr_root,
            start_seq,
            leaf_count,
            leaf_index,
            chunk_index,
            nonce,
            provider_signature,
        );

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        // Wait for finalization and extract challenge ID from events
        let events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        let challenge_id = Self::extract_challenge_id(&events)?;
        tracing::info!(
            "Off-chain challenge created: deadline={}, index={}",
            challenge_id.deadline,
            challenge_id.index
        );

        Ok(challenge_id)
    }

    /// Challenge a replica provider based on their last sync confirmation.
    ///
    /// Replicas confirm syncs on-chain; the chain uses their last confirmed
    /// MMR root as the commitment to challenge against.
    pub async fn challenge_replica(
        &self,
        bucket_id: BucketId,
        provider: String,
        leaf_index: u64,
        chunk_index: u64,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Challenging replica {} on bucket {} (leaf {}, chunk {})",
            provider,
            bucket_id,
            leaf_index,
            chunk_index
        );

        let provider_account = SubstrateClient::parse_account(&provider)?;
        let tx =
            extrinsics::challenge_replica(bucket_id, provider_account, leaf_index, chunk_index);

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        let events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        let challenge_id = Self::extract_challenge_id(&events)?;
        tracing::info!(
            "Replica challenge created: deadline={}, index={}",
            challenge_id.deadline,
            challenge_id.index
        );

        Ok(challenge_id)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring & Strategy
    // ═════════════════════════════════════════════════════════════════════════

    /// Get all active challenges created by this challenger.
    pub async fn list_my_challenges(&self) -> ClientResult<Vec<ChallengeInfo>> {
        let chain = self.base.chain()?;
        let challenger_account = SubstrateClient::parse_account(&self.challenger_account)?;

        let raw = chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?
            .call(
                storage_subxt::api::apis()
                    .storage_provider_api()
                    .challenger_challenges(challenger_account),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("challenger_challenges: {e}")))?;

        Ok(raw
            .into_iter()
            .map(|c| ChallengeInfo {
                challenge_id: ChallengeId {
                    deadline: c.deadline,
                    index: c.index,
                },
                bucket_id: c.bucket_id,
                provider: format!("0x{}", hex::encode(&c.provider)),
                deadline: c.deadline,
                deposit: c.deposit,
                status: ChallengeStatus::Pending,
            })
            .collect())
    }

    /// Analyze a provider to decide whether to challenge them.
    ///
    /// Queries on-chain stats (challenges received/failed, checkpoint age)
    /// and returns a recommendation.
    pub async fn analyze_provider(
        &self,
        bucket_id: BucketId,
        provider: String,
    ) -> ClientResult<ProviderAnalysis> {
        let chain = self.base.chain()?;
        let provider_account = SubstrateClient::parse_account(&provider)?;

        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        // Query provider info for stats
        let provider_thunk = storage
            .fetch(&storage::provider_info(&provider_account))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        let (challenges_received, challenges_failed) = if let Some(p) = provider_thunk {
            (p.stats.challenges_received, p.stats.challenges_failed)
        } else {
            return Err(ClientError::Chain(format!("Provider {provider} not found")));
        };

        // Compute checkpoint age from bucket snapshot
        let last_checkpoint_age = {
            let current_block = chain
                .api()
                .blocks()
                .at_latest()
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to get latest block: {e}")))?
                .number();

            let bucket_thunk = storage
                .fetch(&storage::bucket_info(bucket_id))
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to fetch bucket: {e}")))?;

            if let Some(bucket) = bucket_thunk {
                let checkpoint_block = bucket
                    .snapshot
                    .as_ref()
                    .map(|s| s.checkpoint_block)
                    .unwrap_or(0);
                current_block.saturating_sub(checkpoint_block)
            } else {
                0
            }
        };

        let reputation = reputation_score(challenges_received, challenges_failed);
        let challenge_success_rate = if challenges_received == 0 {
            100.0
        } else {
            let defended = challenges_received.saturating_sub(challenges_failed);
            defended as f64 / challenges_received as f64 * 100.0
        };

        let recommendation = if reputation < 60 {
            ChallengeRecommendation::Challenge
        } else if reputation < 85 {
            ChallengeRecommendation::Monitor
        } else {
            ChallengeRecommendation::Skip
        };

        Ok(ProviderAnalysis {
            provider,
            reputation,
            last_checkpoint_age,
            challenge_success_rate,
            recommendation,
        })
    }

    /// Automated challenge strategy: find and challenge providers with low reputation.
    ///
    /// Iterates all active agreements, scores each provider by on-chain stats,
    /// and submits challenges against the worst performers up to `max_challenges_per_round`.
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

        let chain = self.base.chain()?;

        let rt_api = chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?;

        // Step 1: enumerate all providers and score by reputation; collect those below
        // the threshold along with one of their active agreement bucket_ids.
        const PAGE: u32 = 256;
        let mut scored: Vec<(BucketId, AccountId32, u8)> = Vec::new();
        let mut seen: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
        let mut offset = 0u32;

        loop {
            let page = rt_api
                .call(
                    storage_subxt::api::apis()
                        .storage_provider_api()
                        .providers(offset, PAGE),
                )
                .await
                .map_err(|e| ClientError::Chain(format!("providers: {e}")))?;
            let done = (page.len() as u32) < PAGE;

            for (account, info) in page {
                if !seen.insert(account.0) {
                    continue;
                }
                let rep = reputation_score(info.challenges_received, info.challenges_failed);
                if rep >= min_reputation_threshold {
                    continue;
                }
                let agreements = rt_api
                    .call(
                        storage_subxt::api::apis()
                            .storage_provider_api()
                            .provider_agreements(account.clone()),
                    )
                    .await
                    .map_err(|e| ClientError::Chain(format!("provider_agreements: {e}")))?;
                if let Some(a) = agreements.into_iter().next() {
                    scored.push((a.bucket_id, account, rep));
                }
            }

            if done {
                break;
            }
            offset += PAGE;
        }

        // Sort by worst reputation first
        scored.sort_by_key(|(_, _, rep)| *rep);

        // Step 3: submit challenges
        let mut challenge_ids = Vec::new();

        for (bucket_id, provider_account, _rep) in scored.iter().take(max_challenges_per_round) {
            let signer = chain.signer()?;

            // Challenge leaf 0, chunk 0 as a basic liveness check
            let tx = extrinsics::challenge_checkpoint(*bucket_id, provider_account.clone(), 0, 0);

            let result = chain
                .api()
                .tx()
                .sign_and_submit_then_watch_default(&tx, signer)
                .await;

            match result {
                Ok(progress) => match progress.wait_for_finalized_success().await {
                    Ok(events) => match Self::extract_challenge_id(&events) {
                        Ok(id) => {
                            tracing::info!(
                                "Auto-challenge submitted for bucket {}: deadline={}, index={}",
                                bucket_id,
                                id.deadline,
                                id.index
                            );
                            challenge_ids.push(id);
                        }
                        Err(e) => tracing::warn!("Could not extract challenge ID: {e}"),
                    },
                    Err(e) => tracing::warn!("Challenge tx failed for bucket {bucket_id}: {e}"),
                },
                Err(e) => tracing::warn!("Failed to submit challenge for bucket {bucket_id}: {e}"),
            }
        }

        Ok(challenge_ids)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Analytics
    // ═════════════════════════════════════════════════════════════════════════

    /// Get aggregated stats for this account's challenge activity.
    ///
    /// Pulls counters from on-chain `ChallengerStats`. The pallet maintains
    /// these on `create_challenge`, on `ChallengeDefended`, and on each
    /// `slash_provider_for_failed_challenge` call.
    pub async fn get_challenge_stats(&self) -> ClientResult<ChallengeStats> {
        let stats = self.fetch_challenger_stats().await?;
        Ok(ChallengeStats {
            total_challenges: stats.total_challenges,
            successful_challenges: stats.successful_challenges,
            failed_challenges: stats.failed_challenges,
            // The pallet doesn't yet track an average response time per
            // challenger; leave at 0 until that aggregate is added.
            avg_response_time: 0,
        })
    }

    /// Read this account's `ChallengerStats` record from chain. Returns a
    /// zeroed record (matching the pallet's `ValueQuery` default) if the
    /// account has never opened a challenge.
    async fn fetch_challenger_stats(&self) -> ClientResult<ChallengerStatRecord> {
        let chain = self.base.chain()?;
        let challenger_account = SubstrateClient::parse_account(&self.challenger_account)?;

        chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch_or_default(&storage::challenger_stats(&challenger_account))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch ChallengerStats: {e}")))
    }

    /// Find the most profitable providers to challenge, ranked by expected value.
    ///
    /// Scores all providers by reputation (from on-chain stats) and stake.
    /// Providers with lower reputation and higher stake are ranked highest.
    pub async fn find_challenge_targets(&self, limit: usize) -> ClientResult<Vec<ChallengeTarget>> {
        let chain = self.base.chain()?;

        let rt_api = chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?;

        // Enumerate all providers; score and collect those below the 90-rep threshold.
        const PAGE: u32 = 256;
        let mut targets: Vec<ChallengeTarget> = Vec::new();
        let mut seen: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
        let mut offset = 0u32;

        loop {
            let page = rt_api
                .call(
                    storage_subxt::api::apis()
                        .storage_provider_api()
                        .providers(offset, PAGE),
                )
                .await
                .map_err(|e| ClientError::Chain(format!("providers: {e}")))?;
            let done = (page.len() as u32) < PAGE;

            for (account, info) in page {
                if !seen.insert(account.0) {
                    continue;
                }

                let stake = info.stake;
                let received = info.challenges_received;
                let failed = info.challenges_failed;

                let rep = reputation_score(received, failed);

                // Providers below 90 reputation are worth considering
                if rep >= 90 {
                    continue;
                }

                let agreements = rt_api
                    .call(
                        storage_subxt::api::apis()
                            .storage_provider_api()
                            .provider_agreements(account.clone()),
                    )
                    .await
                    .map_err(|e| ClientError::Chain(format!("provider_agreements: {e}")))?;

                let Some(a) = agreements.into_iter().next() else {
                    continue;
                };

                // Rough reward estimate: ~10% of stake gets slashed on failure
                let potential_reward = stake / 10;

                // Success probability is inverse of their historic defense rate
                let fail_rate = if received == 0 {
                    0.1 // assume 10% base risk for untested providers
                } else {
                    failed as f64 / received as f64
                };
                // Higher fail_rate = higher success probability for challenger
                let success_probability = (fail_rate * 0.8 + 0.1).min(1.0);

                let expected_value = (potential_reward as f64 * success_probability) as u128;

                targets.push(ChallengeTarget {
                    provider: format!("0x{}", hex::encode(account.0)),
                    bucket_id: a.bucket_id,
                    potential_reward,
                    success_probability,
                    expected_value,
                });
            }

            if done {
                break;
            }
            offset += PAGE;
        }

        // Rank by expected value descending
        targets.sort_by(|a, b| b.expected_value.cmp(&a.expected_value));
        targets.truncate(limit);

        Ok(targets)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Internal Helpers
    // ═════════════════════════════════════════════════════════════════════════

    /// Extract ChallengeId from ChallengeCreated event in finalized transaction events.
    fn extract_challenge_id(events: &ExtrinsicEvents<PolkadotConfig>) -> ClientResult<ChallengeId> {
        for event in events.iter() {
            let event =
                event.map_err(|e| ClientError::Chain(format!("Failed to decode event: {e}")))?;
            if let Some(e) = event.as_event::<EvChallengeCreated>().ok().flatten() {
                return Ok(ChallengeId {
                    deadline: e.challenge_id.deadline,
                    index: e.challenge_id.index,
                });
            }
        }

        Err(ClientError::Chain(
            "ChallengeCreated event not found in transaction events".to_string(),
        ))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compute a 0–100 reputation score from on-chain challenge stats.
/// Providers with no recorded challenges score 100 (benefit of the doubt).
fn reputation_score(challenges_received: u32, challenges_failed: u32) -> u8 {
    if challenges_received == 0 {
        return 100;
    }
    let defended = challenges_received.saturating_sub(challenges_failed);
    ((defended as u64 * 100) / challenges_received as u64).min(100) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

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
