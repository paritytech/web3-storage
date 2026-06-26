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
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::BucketId;
use subxt::blocks::ExtrinsicEvents;
use subxt::dynamic::At;
use subxt::ext::scale_value::{Composite, ValueDef};
use subxt::PolkadotConfig;

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
        leaf_index: u64,
        chunk_index: u64,
        provider_signature: Vec<u8>,
    ) -> ClientResult<ChallengeId> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Challenging {} on bucket {} using off-chain commitment (leaf {}, chunk {})",
            provider,
            bucket_id,
            leaf_index,
            chunk_index
        );

        // Parse provider account
        let provider_account = SubstrateClient::parse_account(&provider)?;

        // Create and submit the extrinsic
        let tx = extrinsics::challenge_offchain(
            bucket_id,
            provider_account,
            mmr_root,
            start_seq,
            leaf_index,
            chunk_index,
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
        let challenger_bytes: &[u8] = challenger_account.as_ref();

        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let mut iter = storage
            .iter(storage::all_challenges())
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate challenges: {e}")))?;

        let mut challenges = Vec::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            // Key layout: [twox128(pallet)=16][twox128(storage)=16][blake2_128(block)=16][block=4]
            // deadline block at [48..52]
            let key = &kv.key_bytes;
            if key.len() < 52 {
                continue;
            }
            let deadline = u32::from_le_bytes(key[48..52].try_into().unwrap_or([0u8; 4]));

            let value = match kv.value.to_value() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to decode challenges at block {deadline}: {e}");
                    continue;
                }
            };

            // Value is Vec<Challenge<T>> encoded as unnamed composite
            let challenge_list = match &value.value {
                ValueDef::Composite(Composite::Unnamed(items)) => items.clone(),
                _ => continue,
            };

            for (idx, challenge_val) in challenge_list.iter().enumerate() {
                let ch_bytes = named_field(challenge_val, "challenger")
                    .and_then(decode_account_bytes)
                    .unwrap_or_default();

                if ch_bytes != challenger_bytes {
                    continue;
                }

                let bucket_id = named_field(challenge_val, "bucket_id")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as BucketId;

                let provider_bytes = named_field(challenge_val, "provider")
                    .and_then(decode_account_bytes)
                    .unwrap_or_default();
                let provider = format!("0x{}", hex::encode(&provider_bytes));

                let deposit = named_field(challenge_val, "deposit")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0);

                challenges.push(ChallengeInfo {
                    challenge_id: ChallengeId {
                        deadline,
                        index: idx as u16,
                    },
                    bucket_id,
                    provider,
                    deadline,
                    deposit,
                    status: ChallengeStatus::Pending,
                });
            }
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

        let (challenges_received, challenges_failed) = if let Some(thunk) = provider_thunk {
            let value = thunk
                .to_value()
                .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;

            let stats = named_field(&value, "stats");
            let received = stats
                .and_then(|s| named_field(s, "challenges_received"))
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;
            let failed = stats
                .and_then(|s| named_field(s, "challenges_failed"))
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;
            (received, failed)
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

            if let Some(thunk) = bucket_thunk {
                let value = thunk
                    .to_value()
                    .map_err(|e| ClientError::Chain(format!("Failed to decode bucket: {e}")))?;

                let checkpoint_block = named_field(&value, "snapshot")
                    .and_then(|snap| match &snap.value {
                        ValueDef::Variant(v) if v.name == "Some" => match &v.values {
                            Composite::Unnamed(items) => items.first(),
                            _ => None,
                        },
                        _ => None,
                    })
                    .and_then(|s| named_field(s, "checkpoint_block"))
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as u32;

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

        // Step 1: collect all (bucket_id, provider_bytes) from active agreements
        let mut candidates: Vec<(BucketId, Vec<u8>)> = {
            let storage = chain
                .api()
                .storage()
                .at_latest()
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

            let mut iter = storage
                .iter(storage::all_storage_agreements())
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to iterate agreements: {e}")))?;

            let mut raw: Vec<(BucketId, Vec<u8>)> = Vec::new();
            let mut seen = std::collections::HashSet::new();

            while let Some(result) = iter.next().await {
                let kv = result
                    .map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

                // Key layout: [pallet=16][storage=16][blake2_128(bucket_id)=16][bucket_id=8]
                //             [blake2_128(provider)=16][provider=32]
                let key = &kv.key_bytes;
                if key.len() < 104 {
                    continue;
                }

                let bucket_id =
                    u64::from_le_bytes(key[48..56].try_into().unwrap_or([0u8; 8])) as BucketId;
                let provider_bytes = key[72..104].to_vec();

                if seen.insert(provider_bytes.clone()) {
                    raw.push((bucket_id, provider_bytes));
                }
            }

            raw
        };

        // Step 2: score each provider, keep only those below the threshold
        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let mut scored: Vec<(BucketId, AccountId32, u8)> = Vec::new();

        for (bucket_id, provider_bytes) in &candidates {
            let Ok(arr) = <[u8; 32]>::try_from(provider_bytes.as_slice()) else {
                continue;
            };
            let account = AccountId32::from(arr);

            let Ok(Some(thunk)) = storage.fetch(&storage::provider_info(&account)).await else {
                continue;
            };

            let Ok(value) = thunk.to_value() else {
                continue;
            };

            let stats = named_field(&value, "stats");
            let received = stats
                .and_then(|s| named_field(s, "challenges_received"))
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;
            let failed = stats
                .and_then(|s| named_field(s, "challenges_failed"))
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;

            let rep = reputation_score(received, failed);
            if rep < min_reputation_threshold {
                scored.push((*bucket_id, account, rep));
            }
        }

        // Sort by worst reputation first
        scored.sort_by_key(|(_, _, rep)| *rep);
        candidates.truncate(max_challenges_per_round);

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

        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::challenges(challenge_id.deadline))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch challenges: {e}")))?;

        let Some(thunk) = thunk else {
            // No entry at this deadline block — all challenges there were settled
            tracing::info!(
                "Challenge (deadline={}, index={}) has been settled (no longer in storage)",
                challenge_id.deadline,
                challenge_id.index
            );
            return Ok(None);
        };

        let value = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode challenges: {e}")))?;

        let still_exists = match &value.value {
            ValueDef::Composite(Composite::Unnamed(items)) => {
                (challenge_id.index as usize) < items.len()
            }
            _ => false,
        };

        if still_exists {
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
        let challenger_account = SubstrateClient::parse_account(&self.challenger_account)?;
        let challenger_bytes: &[u8] = challenger_account.as_ref();

        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let query = subxt::dynamic::storage(
            "StorageProvider",
            "ChallengerStats",
            vec![subxt::dynamic::Value::from_bytes(challenger_bytes)],
        );

        let value = match storage
            .fetch(&query)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch ChallengerStats: {e}")))?
        {
            Some(v) => v,
            None => return Ok(FetchedChallengerStats::default()),
        };

        let decoded = value
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Decode ChallengerStats: {e}")))?;

        fn read_u128(value: &subxt::ext::scale_value::Value<u32>, field: &str) -> Option<u128> {
            named_field(value, field).and_then(|v| match &v.value {
                ValueDef::Primitive(subxt::ext::scale_value::Primitive::U128(n)) => Some(*n),
                _ => None,
            })
        }

        Ok(FetchedChallengerStats {
            total_challenges: read_u128(&decoded, "total_challenges").unwrap_or(0) as u32,
            successful_challenges: read_u128(&decoded, "successful_challenges").unwrap_or(0) as u32,
            failed_challenges: read_u128(&decoded, "failed_challenges").unwrap_or(0) as u32,
        })
    }

    /// Find the most profitable providers to challenge, ranked by expected value.
    ///
    /// Scores all providers by reputation (from on-chain stats) and stake.
    /// Providers with lower reputation and higher stake are ranked highest.
    pub async fn find_challenge_targets(&self, limit: usize) -> ClientResult<Vec<ChallengeTarget>> {
        let chain = self.base.chain()?;

        // Step 1: collect unique (bucket_id, provider_bytes) from all agreements
        let candidates: Vec<(BucketId, Vec<u8>)> = {
            let storage = chain
                .api()
                .storage()
                .at_latest()
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

            let mut iter = storage
                .iter(storage::all_storage_agreements())
                .await
                .map_err(|e| ClientError::Chain(format!("Failed to iterate agreements: {e}")))?;

            let mut raw: Vec<(BucketId, Vec<u8>)> = Vec::new();
            let mut seen = std::collections::HashSet::new();

            while let Some(result) = iter.next().await {
                let kv = result
                    .map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

                let key = &kv.key_bytes;
                if key.len() < 104 {
                    continue;
                }

                let bucket_id =
                    u64::from_le_bytes(key[48..56].try_into().unwrap_or([0u8; 8])) as BucketId;
                let provider_bytes = key[72..104].to_vec();

                if seen.insert(provider_bytes.clone()) {
                    raw.push((bucket_id, provider_bytes));
                }
            }

            raw
        };

        // Step 2: score each provider
        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let mut targets: Vec<ChallengeTarget> = Vec::new();

        for (bucket_id, provider_bytes) in &candidates {
            let Ok(arr) = <[u8; 32]>::try_from(provider_bytes.as_slice()) else {
                continue;
            };
            let account = AccountId32::from(arr);

            let Ok(Some(thunk)) = storage.fetch(&storage::provider_info(&account)).await else {
                continue;
            };

            let Ok(value) = thunk.to_value() else {
                continue;
            };

            let stake = named_field(&value, "stake")
                .and_then(|v| v.as_u128())
                .unwrap_or(0);

            let stats = named_field(&value, "stats");
            let received = stats
                .and_then(|s| named_field(s, "challenges_received"))
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;
            let failed = stats
                .and_then(|s| named_field(s, "challenges_failed"))
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;

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
                provider: format!("0x{}", hex::encode(provider_bytes)),
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
        for event in events.iter() {
            let event =
                event.map_err(|e| ClientError::Chain(format!("Failed to decode event: {e}")))?;

            if event.pallet_name() == "StorageProvider"
                && event.variant_name() == "ChallengeCreated"
            {
                let fields = event.field_values().map_err(|e| {
                    ClientError::Chain(format!("Failed to decode event fields: {e}"))
                })?;

                // fields is a scale_value::Value — navigate the composite
                // ChallengeCreated { challenge_id: { deadline, index }, ... }
                let challenge_id_val = fields.at("challenge_id").ok_or_else(|| {
                    ClientError::Chain(
                        "ChallengeCreated event missing challenge_id field".to_string(),
                    )
                })?;

                let deadline = challenge_id_val
                    .at("deadline")
                    .and_then(|v| v.as_u128())
                    .ok_or_else(|| {
                        ClientError::Chain("ChallengeCreated: cannot parse deadline".to_string())
                    })? as u32;

                let index = challenge_id_val
                    .at("index")
                    .and_then(|v| v.as_u128())
                    .ok_or_else(|| {
                        ClientError::Chain("ChallengeCreated: cannot parse index".to_string())
                    })? as u16;

                return Ok(ChallengeId { deadline, index });
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

fn named_field<'a>(
    value: &'a subxt::ext::scale_value::Value<u32>,
    field: &str,
) -> Option<&'a subxt::ext::scale_value::Value<u32>> {
    match &value.value {
        ValueDef::Composite(Composite::Named(fields)) => {
            fields.iter().find(|(n, _)| n == field).map(|(_, v)| v)
        }
        _ => None,
    }
}

fn decode_account_bytes(value: &subxt::ext::scale_value::Value<u32>) -> Option<Vec<u8>> {
    match &value.value {
        ValueDef::Composite(Composite::Unnamed(items)) if items.len() == 32 => {
            items.iter().map(|b| b.as_u128().map(|n| n as u8)).collect()
        }
        _ => None,
    }
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
