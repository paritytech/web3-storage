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
use crate::substrate::{extrinsics, fetch_current_anchor_block, SubstrateClient};
use crate::Signer;
use sp_runtime::AccountId32;
use storage_primitives::{BucketId, ChunkLocation, Commitment};
use storage_subxt::api;
use storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api::ChallengeCandidate;
use storage_subxt::api::storage_provider::events::ChallengeCreated;
use subxt::extrinsics::ExtrinsicEvents;
use subxt::PolkadotConfig;

/// Client for challengers (third parties who verify data integrity).
pub struct ChallengerClient {
    base: BaseClient,
    signer: Signer,
}

/// Top of the reputation scale, mirroring
/// `pallet_storage_provider::runtime_api::reputation_score`, which returns
/// 0..=100. Not importable: this crate depends on the generated bindings
/// rather than on the pallet.
const REPUTATION_SCALE_MAX: u8 = 100;

/// Ceiling the chain applies to `challenge_candidates`, mirroring
/// `pallet_storage_provider::runtime_api::MAX_CHALLENGE_CANDIDATES`. Not
/// importable: this crate depends on the generated bindings rather than on the
/// pallet, so the two must be kept in step by hand.
const MAX_CHALLENGE_CANDIDATES: u32 = 256;

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
    /// - `provider_signature`: The provider's scheme-tagged signature on the
    ///   commitment, as returned by
    ///   [`CommitResponse::provider_signature`](crate::CommitResponse)
    pub async fn challenge_offchain(
        &self,
        bucket_id: BucketId,
        provider: String,
        commitment: Commitment,
        target: ChunkLocation,
        provider_signature: sp_runtime::MultiSignature,
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
            &provider_signature,
        );

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

        let challenges = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .challenger_challenges(challenger),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("challenger_challenges runtime API failed: {e}"))
            })?;

        Ok(challenges
            .into_iter()
            .filter_map(|c| {
                let provider = convert::account_from_runtime_api(&c.provider, "provider")?;
                Some(ChallengeInfo {
                    // The stable index is carried by the response, never
                    // derived from result position.
                    challenge_id: ChallengeId {
                        deadline: c.deadline,
                        index: c.index,
                    },
                    bucket_id: c.bucket_id,
                    provider: convert::account_hex(&provider),
                    deadline: c.deadline,
                    deposit: c.deposit,
                    status: ChallengeStatus::Pending,
                })
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

        let at = chain.at_current_block().await?;

        // Reputation comes from the chain rather than a client-side formula,
        // so it cannot drift from how challenge_candidates ranks providers.
        let provider_info = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .provider_info(convert::to_subxt_account(&provider_account)),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("provider_info runtime API failed: {e}")))?;

        let (challenges_received, challenges_failed, reputation) = if let Some(info) = provider_info
        {
            (
                info.challenges_received,
                info.challenges_failed,
                info.reputation,
            )
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

        // The chain scores every agreement-holding provider and returns the
        // sub-threshold ones worst-first, so there is nothing left to rank here.
        let scored = self
            .challenge_candidates(min_reputation_threshold, max_challenges_per_round as u32)
            .await?;

        let mut challenge_ids = Vec::new();

        for (bucket_id, provider_account, _rep) in scored.iter() {
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

    /// Providers worth challenging: those holding a storage agreement whose
    /// reputation is below `max_reputation`, worst first.
    ///
    /// The chain does the agreement scan, the provider join, the scoring and
    /// the ranking, so this is one call rather than two full-map scans.
    ///
    /// `max_reputation` must be within the 0..=100 reputation scale.
    async fn challenge_candidates(
        &self,
        max_reputation: u8,
        limit: u32,
    ) -> ClientResult<Vec<(BucketId, AccountId32, ChallengeCandidate)>> {
        if max_reputation > REPUTATION_SCALE_MAX {
            return Err(ClientError::Config(format!(
                "max_reputation must be 0..={REPUTATION_SCALE_MAX}, got {max_reputation}"
            )));
        }

        let chain = self.base.chain()?;

        let at = chain.at_current_block().await?;

        let candidates = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .challenge_candidates(max_reputation, limit),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("challenge_candidates runtime API failed: {e}"))
            })?;

        Ok(candidates
            .into_iter()
            .filter_map(|c| {
                let provider = convert::account_from_runtime_api(&c.provider, "provider")?;
                Some((c.bucket_id, convert::to_sp_account(&provider), c))
            })
            .collect())
    }

    /// Find the most profitable providers to challenge, ranked by expected value.
    ///
    /// Scores eligible providers by reputation (from on-chain stats) and stake.
    /// Providers with lower reputation and higher stake are ranked highest.
    ///
    /// `max_reputation` bounds eligibility: a provider qualifies when its
    /// reputation is strictly below it. The chain also caps the eligible pool
    /// at its own ceiling, worst-reputation first, so this can miss a
    /// high-stake provider outside that cap.
    pub async fn find_challenge_targets(
        &self,
        max_reputation: u8,
        limit: usize,
    ) -> ClientResult<Vec<ChallengeTarget>> {
        // The chain returns the worst-by-reputation candidates up to this cap.
        // Reputation is the inverse of failure rate, so that ordering covers
        // half of the expected value below; the half it misses is stake.
        let candidates = self
            .challenge_candidates(max_reputation, MAX_CHALLENGE_CANDIDATES)
            .await?;

        let mut targets: Vec<ChallengeTarget> = Vec::new();

        for (bucket_id, provider, candidate) in &candidates {
            let stake = candidate.stake;
            let received = candidate.challenges_received;
            let failed = candidate.challenges_failed;

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
                provider: convert::account_hex(&convert::to_subxt_account(provider)),
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
