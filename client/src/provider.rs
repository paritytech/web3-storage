// SPDX-License-Identifier: Apache-2.0

//! Provider Client - For storage providers managing their operations.
//!
//! This client provides operations for:
//! - Registering as a provider
//! - Managing provider settings (pricing, availability)
//! - Accepting storage agreements
//! - Responding to challenges
//! - Monitoring earnings and performance

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::substrate::{constants, extrinsics, storage, SubstrateClient};
use rt::pallet_storage_provider::pallet::ProviderInfo;
use rt::pallet_storage_provider::pallet::ProviderSettings;
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types as rt;
use storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api as rt_api;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt::utils::H256;
use storage_subxt::subxt_signer;

/// Client for storage providers.
pub struct ProviderClient {
    base: BaseClient,
    provider_account: String, // Substrate account ID
}

impl ProviderClient {
    /// Create a new provider client.
    pub fn new(config: ClientConfig, provider_account: String) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            provider_account,
        })
    }

    /// Create with default configuration.
    pub fn with_defaults(provider_account: String) -> ClientResult<Self> {
        Self::new(ClientConfig::default(), provider_account)
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

    /// Set a custom keypair signer loaded from a keyfile or seed.
    /// Must be called after connect().
    pub fn set_signer(&mut self, signer: subxt_signer::sr25519::Keypair) -> ClientResult<()> {
        self.base.set_signer(signer)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Provider Registration & Settings
    // ═════════════════════════════════════════════════════════════════════════

    /// Register as a storage provider on-chain.
    ///
    /// This creates a provider profile with initial settings.
    ///
    /// # Parameters
    /// - `multiaddr`: Network address for clients to connect (e.g., "/ip4/1.2.3.4/tcp/3333")
    /// - `public_key`: Public key for signature verification (32-64 bytes)
    /// - `stake`: Initial stake to lock (in smallest unit)
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::ProviderClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ProviderClient::with_defaults("5GrwvaEF...".to_string())?;
    /// let multiaddr = "/ip4/203.0.113.1/tcp/3333".to_string();
    /// let public_key = vec![0u8; 32]; // Your actual public key
    /// let stake = 1_000_000_000_000u128; // 1 token with 12 decimals
    ///
    /// client.register(multiaddr, public_key, stake).await?;
    /// println!("Successfully registered as provider!");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn register(
        &self,
        multiaddr: String,
        public_key: Vec<u8>,
        stake: u128,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Registering provider {} with stake {}",
            self.provider_account,
            stake
        );

        // Create the extrinsic
        let tx = extrinsics::register_provider(multiaddr.into_bytes(), public_key, stake);

        // Submit and wait for inclusion
        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        // Wait for finalization
        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Provider registered successfully");
        Ok(())
    }

    /// Query a provider's current info from the chain.
    ///
    /// Returns `None` if the provider is not registered.
    pub async fn get_provider_info(
        &self,
        account: &AccountId32,
    ) -> ClientResult<Option<ProviderInfo>> {
        let chain = self.base.chain()?;

        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::provider_info(account))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        Ok(thunk)
    }

    /// Update provider settings.
    ///
    /// Change pricing, availability, or other settings.
    pub async fn update_settings(&self, settings: ProviderSettings) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Updating settings for provider {}: price_per_byte={}",
            self.provider_account,
            settings.price_per_byte
        );

        let tx = extrinsics::update_provider_settings(settings);

        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Provider settings updated");
        Ok(())
    }

    /// Add more stake to your provider account.
    pub async fn add_stake(&self, additional_stake: u128) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Adding stake {} for provider {}",
            additional_stake,
            self.provider_account
        );

        let tx = extrinsics::add_stake(additional_stake);

        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Stake added successfully");
        Ok(())
    }

    /// Deregister as a provider (requires no active agreements).
    pub async fn deregister(&self) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!("Deregistering provider {}", self.provider_account);

        let tx = extrinsics::deregister_provider();

        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Provider deregistered successfully");
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Term Negotiation (off-chain)
    // ═════════════════════════════════════════════════════════════════════════

    /// Read a provider's on-chain `ProviderReplayState.hsn`. Returns
    /// `Ok(None)` if the provider has no replay state yet (never signed
    /// any terms).
    pub async fn fetch_replay_hsn(
        chain_ws_url: &str,
        provider: &AccountId32,
    ) -> ClientResult<Option<u64>> {
        let chain = SubstrateClient::connect(chain_ws_url).await?;
        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::provider_replay_state(provider))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch replay state: {e}")))?;

        let Some(replay) = thunk else {
            return Ok(None);
        };
        Ok(Some(replay.hsn))
    }

    /// Read the chain's `StorageProvider::RequestTimeout` runtime constant —
    /// the validity window (in blocks) applied to provider-signed terms.
    ///
    /// Returns `Ok(None)` if the constant is absent from the node's metadata.
    pub async fn fetch_request_timeout(chain_ws_url: &str) -> ClientResult<Option<u32>> {
        let chain = SubstrateClient::connect(chain_ws_url).await?;
        let value = chain
            .api()
            .constants()
            .at(&constants::request_timeout())
            .map_err(|e| ClientError::Chain(format!("Failed to read RequestTimeout: {e}")))?;

        Ok(Some(value))
    }

    /// Negotiate provider-signed agreement terms over HTTP.
    ///
    /// Owner posts the proposed shape; the provider node allocates nonce + validity window from
    /// its own state, signs, returns a [`SignedTerms`](crate::agreement::SignedTerms) ready for
    /// [`AdminClient::establish_storage_agreement`](crate::admin::AdminClient::establish_storage_agreement).
    pub async fn negotiate_terms(
        provider_url: &str,
        req: &crate::agreement::NegotiateRequest,
    ) -> ClientResult<crate::agreement::SignedTerms> {
        let url = format!("{}/negotiate", provider_url.trim_end_matches('/'));
        let response = reqwest::Client::new()
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(ClientError::Http)?;

        if !response.status().is_success() {
            return Err(ClientError::Chain(format!(
                "provider node rejected /negotiate with status {}",
                response.status()
            )));
        }

        response
            .json::<crate::agreement::SignedTerms>()
            .await
            .map_err(ClientError::Http)
    }

    /// Fetch a provider node's identity from its `/info` HTTP endpoint.
    ///
    /// Returns the provider's account as an [`AccountId32`], parsed from the
    /// SS58 string the node reports. Useful for discovering the provider's
    /// on-chain account without hardcoding it.
    pub async fn fetch_provider_id(provider_url: &str) -> ClientResult<AccountId32> {
        let url = format!("{}/info", provider_url.trim_end_matches('/'));
        let response = reqwest::Client::new()
            .get(&url)
            .send()
            .await
            .map_err(ClientError::Http)?;

        if !response.status().is_success() {
            return Err(ClientError::Chain(format!(
                "provider node rejected /info with status {}",
                response.status()
            )));
        }

        let info: serde_json::Value = response.json().await.map_err(ClientError::Http)?;

        let provider_id = info["provider_id"].as_str().ok_or_else(|| {
            ClientError::Chain("provider /info response missing string `provider_id` field".into())
        })?;

        SubstrateClient::parse_account(provider_id)
            .map_err(|e| ClientError::Chain(format!("invalid provider_id from /info: {e}")))
    }

    /// List all active agreements for this provider.
    pub async fn list_active_agreements(&self) -> ClientResult<Vec<ActiveAgreement>> {
        let chain = self.base.chain()?;
        let provider_account = SubstrateClient::parse_account(&self.provider_account)
            .map_err(|e| ClientError::Chain(format!("Invalid provider account: {e}")))?;

        let raw = chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?
            .call(
                storage_subxt::api::apis()
                    .storage_provider_api()
                    .provider_agreements(provider_account),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("provider_agreements: {e}")))?;

        Ok(raw
            .into_iter()
            .map(|a| ActiveAgreement {
                bucket_id: a.bucket_id,
                agreement: a,
            })
            .collect())
    }

    /// Confirm replica sync to receive payment.
    ///
    /// For replica providers, this confirms you've synced data and
    /// triggers payment from the sync_balance.
    pub async fn confirm_replica_sync(
        &self,
        bucket_id: BucketId,
        mmr_roots: [Option<H256>; 7],
        signature: Vec<u8>,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Confirming replica sync for bucket {} with {} roots",
            bucket_id,
            mmr_roots.iter().filter(|r| r.is_some()).count()
        );

        let tx = extrinsics::confirm_replica_sync(bucket_id, mmr_roots, signature);

        chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Replica sync confirmed successfully");
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Challenge Response
    // ═════════════════════════════════════════════════════════════════════════

    /// Respond to a challenge by providing the requested data and proofs.
    ///
    /// # Parameters
    /// - `challenge_id`: (deadline_block, index) identifying the challenge
    /// - `chunk_data`: The actual chunk data being proven
    /// - `mmr_proof`: MMR proof showing the leaf is in the committed MMR
    /// - `chunk_proof`: Merkle proof showing the chunk is in the leaf's data tree
    pub async fn respond_to_challenge(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: &storage_primitives::MmrProof,
        chunk_proof: &storage_primitives::MerkleProof,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Responding to challenge {:?} with {} bytes",
            challenge_id,
            chunk_data.len()
        );

        let tx = extrinsics::respond_to_challenge_proof(
            challenge_id,
            &chunk_data,
            mmr_proof,
            chunk_proof,
        );

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {e}")))?;

        tracing::info!("Challenge response submitted successfully");
        Ok(())
    }

    /// List all active challenges against this provider.
    pub async fn list_active_challenges(&self) -> ClientResult<Vec<ChallengeInfo>> {
        let chain = self.base.chain()?;
        let provider_account = SubstrateClient::parse_account(&self.provider_account)
            .map_err(|e| ClientError::Chain(format!("Invalid provider account: {e}")))?;

        let raw = chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?
            .call(
                storage_subxt::api::apis()
                    .storage_provider_api()
                    .provider_challenges(provider_account),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("provider_challenges: {e}")))?;

        Ok(raw
            .into_iter()
            .map(|c| ChallengeInfo {
                challenge_id: (c.deadline, c.index),
                challenge: c,
            })
            .collect())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring & Analytics
    // ═════════════════════════════════════════════════════════════════════════

    /// Get your provider statistics.
    pub async fn get_stats(&self) -> ClientResult<ProviderStats> {
        let chain = self.base.chain()?;
        let provider_account = SubstrateClient::parse_account(&self.provider_account)
            .map_err(|e| ClientError::Chain(format!("Invalid provider account: {e}")))?;

        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::provider_info(&provider_account))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        let Some(p) = thunk else {
            return Ok(ProviderStats::default());
        };

        let reputation = if p.stats.agreements_total > 0 {
            let failure_rate = p.stats.challenges_failed as f64 / p.stats.agreements_total as f64;
            ((1.0 - failure_rate) * 100.0).clamp(0.0, 100.0) as u8
        } else {
            100
        };

        Ok(ProviderStats {
            stake: p.stake,
            committed_bytes: p.committed_bytes,
            agreements_total: p.stats.agreements_total,
            agreements_extended: p.stats.agreements_extended,
            challenges_received: p.stats.challenges_received,
            challenges_failed: p.stats.challenges_failed,
            reputation,
        })
    }

    /// Get your total earnings (all time).
    ///
    /// Note: historical earnings are not stored on-chain; this returns 0.
    pub async fn get_total_earnings(&self) -> ClientResult<u128> {
        Ok(0)
    }

    /// Get your current committed bytes vs available capacity.
    pub async fn get_capacity_info(&self) -> ClientResult<CapacityInfo> {
        let chain = self.base.chain()?;
        let provider_account = SubstrateClient::parse_account(&self.provider_account)
            .map_err(|e| ClientError::Chain(format!("Invalid provider account: {e}")))?;

        let thunk = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::provider_info(&provider_account))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        let Some(p) = thunk else {
            return Ok(CapacityInfo {
                committed_bytes: 0,
                available_bytes: 0,
                stake: 0,
                required_stake: 0,
            });
        };

        let available_bytes = p.settings.max_capacity.saturating_sub(p.committed_bytes);

        Ok(CapacityInfo {
            committed_bytes: p.committed_bytes,
            available_bytes,
            stake: p.stake,
            required_stake: 0,
        })
    }

    /// Monitor reputation score.
    pub async fn get_reputation(&self) -> ClientResult<u8> {
        let stats = self.get_stats().await?;
        Ok(stats.reputation)
    }
}

// Types

#[derive(Debug, Clone)]
pub struct ActiveAgreement {
    pub bucket_id: BucketId,
    pub agreement: rt_api::AgreementResponse,
}

#[derive(Debug, Clone)]
pub struct ChallengeInfo {
    pub challenge_id: (u32, u16),
    pub challenge: rt_api::ChallengeResponse,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderStats {
    pub stake: u128,
    pub committed_bytes: u64,
    pub agreements_total: u32,
    pub agreements_extended: u32,
    pub challenges_received: u32,
    pub challenges_failed: u32,
    pub reputation: u8,
}

#[derive(Debug, Clone)]
pub struct CapacityInfo {
    pub committed_bytes: u64,
    pub available_bytes: u64,
    pub stake: u128,
    pub required_stake: u128,
}
