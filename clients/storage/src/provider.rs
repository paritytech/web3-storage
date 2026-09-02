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
use crate::convert;
use crate::discovery::ProviderInfo;
use crate::substrate::{extrinsics, SubstrateClient};
use crate::Signer;
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::BucketId;
use storage_subxt::api;
use storage_subxt::api::runtime_types::storage_primitives::ProviderRole;

/// Client for storage providers.
pub struct ProviderClient {
    base: BaseClient,
    signer: Signer,
}

impl ProviderClient {
    /// Create a new provider client. `signer` submits every extrinsic and
    /// identifies the provider account.
    pub fn new(config: ClientConfig, signer: Signer) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            signer,
        })
    }

    /// The provider account: the signer's public key.
    fn provider_account(&self) -> AccountId32 {
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
    /// # use storage_client::{ProviderClient, Signer};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ProviderClient::with_defaults(Signer::from_seed("//Alice")?)?;
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
            self.provider_account(),
            stake
        );

        // Create the extrinsic
        let tx = extrinsics::register_provider(multiaddr.into_bytes(), public_key, stake);

        // Submit and wait for inclusion
        let tx_progress = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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

        let at = chain.at_current_block().await?;
        let info = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .provider_info(convert::to_subxt_account(account)),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("provider_info runtime API failed: {e}")))?;

        Ok(info.map(ProviderInfo::from))
    }

    /// Update provider settings.
    ///
    /// Change pricing, availability, or other settings.
    pub async fn update_settings(&self, settings: ProviderSettings) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Updating settings for provider {}: price_per_byte={}",
            self.provider_account(),
            settings.price_per_byte
        );

        let tx = extrinsics::update_provider_settings(
            settings.min_duration,
            settings.max_duration,
            settings.price_per_byte,
            settings.accepting_primary,
            settings.replica_sync_price,
            settings.accepting_extensions,
            settings.max_capacity,
        );

        chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            self.provider_account()
        );

        let tx = extrinsics::add_stake(additional_stake);

        chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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

        tracing::info!("Deregistering provider {}", self.provider_account());

        let tx = extrinsics::deregister_provider();

        chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
        let at = chain.at_current_block().await?;
        let value = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().provider_replay_states(),
                (convert::to_subxt_account(provider),),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch replay state: {e}")))?;

        let Some(value) = value else {
            return Ok(None);
        };
        let window = value
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode replay state: {e}")))?;
        Ok(Some(window.hsn))
    }

    /// Read the chain's `StorageProvider::RequestTimeout` runtime constant —
    /// the validity window (in blocks) applied to provider-signed terms.
    ///
    /// Errors if the node's metadata does not carry the constant (i.e. the
    /// generated bindings have drifted from the runtime).
    pub async fn fetch_request_timeout(chain_ws_url: &str) -> ClientResult<u32> {
        let chain = SubstrateClient::connect(chain_ws_url).await?;
        let value = chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to read RequestTimeout: {e}")))?
            .constants()
            .entry(
                storage_subxt::api::constants()
                    .storage_provider()
                    .request_timeout(),
            )
            .map_err(|e| ClientError::Chain(format!("Failed to decode RequestTimeout: {e}")))?;

        Ok(value)
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
        let provider = convert::to_subxt_account(&self.provider_account());

        let at = chain.at_current_block().await?;

        let agreements = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .provider_agreements(provider),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("provider_agreements runtime API failed: {e}"))
            })?;

        Ok(agreements
            .into_iter()
            .filter_map(|a| {
                let owner = convert::account_from_runtime_api(&a.owner, "owner")?;
                Some(ActiveAgreement {
                    bucket_id: a.bucket_id,
                    owner: convert::account_hex(&owner),
                    max_bytes: a.max_bytes,
                    expires_at: a.expires_at,
                    is_primary: matches!(a.role, ProviderRole::Primary),
                })
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

        let tx = extrinsics::confirm_replica_sync(bucket_id, mmr_roots, signature)?;

        chain
            .api()
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
            .at_current_block()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {e}")))?
            .transactions()
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
        let provider = convert::to_subxt_account(&self.provider_account());

        let at = chain.at_current_block().await?;

        let challenges = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .provider_challenges(provider),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("provider_challenges runtime API failed: {e}"))
            })?;

        Ok(challenges
            .into_iter()
            .map(|c| ChallengeInfo {
                // The stable index is carried by the response, never derived
                // from result position.
                challenge_id: (c.deadline, c.index),
                bucket_id: c.bucket_id,
                deadline: c.deadline,
                leaf_index: c.target.leaf_index,
                chunk_index: c.target.chunk_index,
            })
            .collect())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring & Analytics
    // ═════════════════════════════════════════════════════════════════════════

    /// Get your provider statistics.
    pub async fn get_stats(&self) -> ClientResult<ProviderStats> {
        let chain = self.base.chain()?;
        let provider_account = self.provider_account();

        let at = chain.at_current_block().await?;
        let value = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().providers(),
                (convert::to_subxt_account(&provider_account),),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        let Some(value) = value else {
            return Ok(ProviderStats::default());
        };

        let info = value
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;

        let reputation = if info.stats.agreements_total > 0 {
            let failure_rate =
                info.stats.challenges_failed as f64 / info.stats.agreements_total as f64;
            ((1.0 - failure_rate) * 100.0).clamp(0.0, 100.0) as u8
        } else {
            100
        };

        Ok(ProviderStats {
            stake: info.stake,
            committed_bytes: info.committed_bytes,
            agreements_total: info.stats.agreements_total,
            agreements_extended: info.stats.agreements_extended,
            challenges_received: info.stats.challenges_received,
            challenges_failed: info.stats.challenges_failed,
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
        let provider_account = self.provider_account();

        let at = chain.at_current_block().await?;
        let value = at
            .storage()
            .try_fetch(
                api::storage().storage_provider().providers(),
                (convert::to_subxt_account(&provider_account),),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?;

        let Some(value) = value else {
            return Ok(CapacityInfo {
                committed_bytes: 0,
                available_bytes: 0,
                stake: 0,
                required_stake: 0,
            });
        };

        let info = value
            .decode()
            .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;

        let available_bytes = info
            .settings
            .max_capacity
            .saturating_sub(info.committed_bytes);

        Ok(CapacityInfo {
            committed_bytes: info.committed_bytes,
            available_bytes,
            stake: info.stake,
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
pub struct ProviderSettings {
    pub price_per_byte: u128,
    pub min_duration: u32,
    pub max_duration: u32,
    pub accepting_primary: bool,
    pub replica_sync_price: Option<u128>,
    pub accepting_extensions: bool,
    /// Maximum storage capacity in bytes. 0 = unlimited.
    pub max_capacity: u64,
}

#[derive(Debug, Clone)]
pub struct AgreementRequest {
    pub bucket_id: BucketId,
    pub requester: String,
    pub max_bytes: u64,
    pub payment_locked: u128,
    pub duration: u32,
    pub expires_at: u32,
}

#[derive(Debug, Clone)]
pub struct ActiveAgreement {
    pub bucket_id: BucketId,
    pub owner: String,
    pub max_bytes: u64,
    pub expires_at: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone)]
pub struct ChallengeInfo {
    pub challenge_id: (u32, u16),
    pub bucket_id: BucketId,
    pub deadline: u32,
    pub leaf_index: u64,
    pub chunk_index: u64,
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
