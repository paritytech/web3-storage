// SPDX-License-Identifier: Apache-2.0

//! Challenger Client - For third parties verifying data integrity.
//!
//! This client provides operations for:
//! - Monitoring provider performance
//! - Creating challenges to verify data availability
//! - Collecting rewards from successful challenges
//! - Automated challenge strategies

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::convert;
use crate::substrate::{decoded_key, extrinsics, fetch_current_anchor_block, SubstrateClient};
use crate::Signer;
use sp_runtime::AccountId32;
use std::collections::HashMap;
use storage_primitives::{BucketId, ChunkLocation, Commitment};
use storage_subxt::api;
use storage_subxt::api::storage_provider::events::ChallengeCreated;
use subxt::extrinsics::ExtrinsicEvents;
use subxt::PolkadotConfig;

/// Client for challengers (third parties who verify data integrity).
pub struct ChallengerClient {
    base: BaseClient,
    signer: Signer,
}

impl ChallengerClient {
    /// Create a new challenger client. `signer` submits every extrinsic and
    /// identifies the challenger account.
    pub fn new(config: ClientConfig, signer: Signer) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            signer,
        })
    }

    /// The challenger account: the signer's public key.
    fn challenger_account(&self) -> AccountId32 {
        AccountId32::new(self.signer.keypair().public_key().0)
    }

    /// Create with default configuration.
    pub fn with_defaults(signer: Signer) -> ClientResult<Self> {
        Self::new(ClientConfig::default(), signer)
    }

    /// Connect to the blockchain and install the signer. Must be called before
    /// any on-chain operations.
    pub async fn connect(&mut self) -> ClientResult<()> {
        self.base.connect_chain().await?;
        self.base.set_signer(self.signer.clone())
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
    /// - `target`: Which leaf + chunk within the MMR to challenge
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::{ChallengerClient, ChunkLocation, Signer};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ChallengerClient::with_defaults(Signer::from_seed("//Alice")?)?;
    ///
    /// // Challenge a random chunk
    /// let bucket_id = 1;
    /// let provider = "5FHneW46...".to_string();
    /// let leaf_index = 5;
    /// let chunk_index = 123;
    ///
    /// client.challenge_checkpoint(bucket_id, provider, ChunkLocation { leaf_index, chunk_index }).await?;
    /// println!("Challenge created!");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn challenge_checkpoint(
        &self,
        bucket_id: BucketId,
        provider: String,
        target: ChunkLocation,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Challenging {} on bucket {} checkpoint (leaf {}, chunk {})",
            provider,
            bucket_id,
            target.leaf_index,
            target.chunk_index
        );

        // Parse provider account
        let provider_account = SubstrateClient::parse_account(&provider)?;

        // Create and submit the extrinsic
        let tx = extrinsics::challenge_checkpoint(bucket_id, provider_account, target);

        let tx_progress = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
    /// - `commitment`: The MMR commitment (root + range) the provider signed over
    /// - `target`: Which leaf + chunk within that commitment to challenge
    /// - `nonce`: The nonce the provider signed over (echoed from their commitment)
    /// - `provider_signature`: The provider's signature on the commitment (64 bytes for Sr25519)
    pub async fn challenge_offchain(
        &self,
        bucket_id: BucketId,
        provider: String,
        commitment: Commitment,
        target: ChunkLocation,
        nonce: u64,
        provider_signature: Vec<u8>,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Challenging {} on bucket {} using off-chain commitment (leaf {}, chunk {})",
            provider,
            bucket_id,
            target.leaf_index,
            target.chunk_index
        );

        // Parse provider account
        let provider_account = SubstrateClient::parse_account(&provider)?;

        // Create and submit the extrinsic
        let tx = extrinsics::challenge_offchain(
            bucket_id,
            provider_account,
            commitment,
            target,
            nonce,
            provider_signature,
        )?;

        let tx_progress = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
        target: ChunkLocation,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Challenging replica {} on bucket {} (leaf {}, chunk {})",
            provider,
            bucket_id,
            target.leaf_index,
            target.chunk_index
        );

        let provider_account = SubstrateClient::parse_account(&provider)?;
        let tx = extrinsics::challenge_replica(bucket_id, provider_account, target);

        let tx_progress = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
        let challenger = convert::to_subxt_account(&self.challenger_account());

        let at = chain.at_current_block().await?;

        let mut iter = at
            .storage()
            .iter(api::storage().storage_provider().challenges(), ())
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate challenges: {e}")))?;

        let mut challenges = Vec::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            // Challenges is a double map (deadline, index) -> Challenge; the
            // stable index comes from the storage key, never from iteration
            // order.
            let (deadline, index): (u32, u16) = match decoded_key(&kv, "challenge") {
                Some(k) => k,
                None => continue,
            };

            let challenge = match kv.value().decode() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to decode challenge at block {deadline}: {e}");
                    continue;
                }
            };

            if challenge.challenger != challenger {
                continue;
            }

            challenges.push(ChallengeInfo {
                challenge_id: ChallengeId { deadline, index },
                bucket_id: challenge.bucket_id,
                provider: convert::account_hex(&challenge.provider),
                deadline,
                deposit: challenge.deposit,
                status: ChallengeStatus::Pending,
            });
        }

        Ok(challenges)
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

        let at = chain.at_current_block().await?;

        // Query provider info for stats
        let provider_value = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().providers(),
                (convert::to_subxt_account(&provider_account),),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        let (challenges_received, challenges_failed) = if let Some(value) = provider_value {
            let info = value
                .decode()
                .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;
            (info.stats.challenges_received, info.stats.challenges_failed)
        } else {
            return Err(ClientError::Chain(format!("Provider {provider} not found")));
        };

        // Compute checkpoint age from bucket snapshot
        let last_checkpoint_age = {
            // `checkpoint_block` is on the pallet's anchor clock (relay
            // blocks), so measure the age on that clock, not the parachain
            // height.
            let anchor_block = fetch_current_anchor_block(&at).await?;

            let bucket_value = at
                .storage()
                .try_fetch(api::storage().storage_provider().buckets(), (bucket_id,))
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to fetch bucket: {e}")))?;

            if let Some(value) = bucket_value {
                let bucket = value
                    .decode()
                    .map_err(|e| ClientError::Chain(format!("Failed to decode bucket: {e}")))?;

                let checkpoint_block = bucket
                    .snapshot
                    .map(|s| s.checkpoint_block)
                    .unwrap_or_default();

                anchor_block.saturating_sub(checkpoint_block)
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

        // Step 1: collect unique (bucket_id, provider) pairs from active agreements
        let candidates = self.agreement_providers().await?;

        // Step 2: score each provider, keep only those below the threshold
        let registered = self.provider_stats().await?;

        let mut scored: Vec<(BucketId, AccountId32, u8)> = Vec::new();

        for (bucket_id, provider) in &candidates {
            let Some(score) = registered.get(&provider.0) else {
                continue;
            };

            let rep = reputation_score(score.challenges_received, score.challenges_failed);
            if rep < min_reputation_threshold {
                scored.push((*bucket_id, convert::to_sp_account(provider), rep));
            }
        }

        // Sort by worst reputation first
        scored.sort_by_key(|(_, _, rep)| *rep);

        // Step 3: submit challenges
        let mut challenge_ids = Vec::new();

        for (bucket_id, provider_account, _rep) in scored.iter().take(max_challenges_per_round) {
            let signer = chain.signer()?;

            // Challenge leaf 0, chunk 0 as a basic liveness check
            let tx = extrinsics::challenge_checkpoint(
                *bucket_id,
                provider_account.clone(),
                ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
            );

            let at = match chain.api().at_current_block().await {
                Ok(at) => at,
                Err(e) => {
                    tracing::warn!("Failed to submit challenge for bucket {bucket_id}: {e}");
                    continue;
                }
            };

            let result = at
                .transactions()
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

    /// Check if a challenge has been settled.
    ///
    /// Challenge rewards are distributed automatically by the chain in `on_finalize`
    /// when the response deadline passes without a valid response from the provider.
    /// Returns `None` if the challenge is still pending or has already been settled
    /// (reward auto-distributed). The exact reward amount is not available on-chain
    /// after settlement without querying historical events.
    pub async fn check_and_claim_reward(
        &self,
        challenge_id: ChallengeId,
    ) -> ClientResult<Option<u128>> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;
        // Challenges is a double map (deadline, index) -> Challenge: a point
        // fetch answers pending-vs-settled directly.
        let entry = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().challenges(),
                (challenge_id.deadline, challenge_id.index),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch challenges: {e}")))?;

        if entry.is_some() {
            tracing::info!(
                "Challenge (deadline={}, index={}) is still pending",
                challenge_id.deadline,
                challenge_id.index
            );
            Ok(None) // Pending — no reward yet
        } else {
            tracing::info!(
                "Challenge (deadline={}, index={}) has been settled",
                challenge_id.deadline,
                challenge_id.index
            );
            Ok(None) // Settled — reward was auto-distributed on-chain
        }
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
    async fn fetch_challenger_stats(&self) -> ClientResult<FetchedChallengerStats> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;

        let value = match at
            .storage()
            .try_fetch(
                api::storage().storage_provider().challenger_stats(),
                (convert::to_subxt_account(&self.challenger_account()),),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch ChallengerStats: {e}")))?
        {
            Some(v) => v,
            None => return Ok(FetchedChallengerStats::default()),
        };

        let record = value
            .decode()
            .map_err(|e| ClientError::Chain(format!("Decode ChallengerStats: {e}")))?;

        Ok(FetchedChallengerStats {
            total_challenges: record.total_challenges,
            successful_challenges: record.successful_challenges,
            failed_challenges: record.failed_challenges,
        })
    }

    /// List every provider that holds a storage agreement, paired with **one**
    /// of its buckets — whichever is seen first while iterating. Each provider
    /// appears exactly once, so callers challenge a provider at most once per
    /// round.
    async fn agreement_providers(
        &self,
    ) -> ClientResult<Vec<(BucketId, subxt::utils::AccountId32)>> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;

        let mut iter = at
            .storage()
            .iter(api::storage().storage_provider().storage_agreements(), ())
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate agreements: {e}")))?;

        let mut providers: Vec<(BucketId, subxt::utils::AccountId32)> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            let (bucket_id, provider): (BucketId, subxt::utils::AccountId32) =
                match decoded_key(&kv, "agreement") {
                    Some(k) => k,
                    None => continue,
                };

            if seen.insert(provider.0) {
                providers.push((bucket_id, provider));
            }
        }

        Ok(providers)
    }

    /// Reputation inputs for every registered provider, keyed by account bytes.
    ///
    /// One paged scan of `Providers` rather than a point fetch per candidate:
    /// the callers below score every agreement holder, so the whole map is
    /// wanted anyway.
    async fn provider_stats(&self) -> ClientResult<HashMap<[u8; 32], ProviderScore>> {
        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;

        let mut iter = at
            .storage()
            .iter(api::storage().storage_provider().providers(), ())
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate providers: {e}")))?;

        let mut stats = HashMap::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            let (account,): (subxt::utils::AccountId32,) = match decoded_key(&kv, "provider") {
                Some(k) => k,
                None => continue,
            };

            match kv.value().decode() {
                Ok(info) => {
                    stats.insert(
                        account.0,
                        ProviderScore {
                            stake: info.stake,
                            challenges_received: info.stats.challenges_received,
                            challenges_failed: info.stats.challenges_failed,
                        },
                    );
                }
                Err(e) => tracing::warn!("Failed to decode provider {}: {e}", account.0[0]),
            }
        }

        Ok(stats)
    }

    /// Find the most profitable providers to challenge, ranked by expected value.
    ///
    /// Scores all providers by reputation (from on-chain stats) and stake.
    /// Providers with lower reputation and higher stake are ranked highest.
    pub async fn find_challenge_targets(&self, limit: usize) -> ClientResult<Vec<ChallengeTarget>> {
        // Step 1: collect unique (bucket_id, provider) pairs from all agreements
        let candidates = self.agreement_providers().await?;

        // Step 2: score each provider
        let registered = self.provider_stats().await?;

        let mut targets: Vec<ChallengeTarget> = Vec::new();

        for (bucket_id, provider) in &candidates {
            let Some(score) = registered.get(&provider.0) else {
                continue;
            };

            let stake = score.stake;
            let received = score.challenges_received;
            let failed = score.challenges_failed;

            let rep = reputation_score(received, failed);

            // Providers below 90 reputation are worth considering
            if rep >= 90 {
                continue;
            }

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
                provider: convert::account_hex(provider),
                bucket_id: *bucket_id,
                potential_reward,
                success_probability,
                expected_value,
            });
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
        let created = events
            .find_first::<ChallengeCreated>()
            .ok_or_else(|| {
                ClientError::Chain(
                    "ChallengeCreated event not found in transaction events".to_string(),
                )
            })?
            .map_err(|e| {
                ClientError::Chain(format!("Failed to decode ChallengeCreated event: {e}"))
            })?;

        Ok(ChallengeId {
            deadline: created.challenge_id.deadline,
            index: created.challenge_id.index,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

/// The `Providers` fields the challenge-scoring paths read.
struct ProviderScore {
    stake: u128,
    challenges_received: u32,
    challenges_failed: u32,
}

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

/// Internal: the raw `ChallengerStatRecord` shape pulled from chain.
/// Public callers see `ChallengeStats` which wraps these counters with the
/// `avg_response_time` field the SDK historically exposed.
#[derive(Debug, Clone, Default)]
struct FetchedChallengerStats {
    total_challenges: u32,
    successful_challenges: u32,
    failed_challenges: u32,
}

#[derive(Debug, Clone)]
pub struct ChallengeTarget {
    pub provider: String,
    pub bucket_id: BucketId,
    pub potential_reward: u128,
    pub success_probability: f64,
    pub expected_value: u128,
}
