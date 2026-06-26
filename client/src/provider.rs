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
use crate::discovery::ProviderInfo;
use crate::substrate::{constants, extrinsics, storage, SubstrateClient};
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::BucketId;
use subxt::ext::scale_value::{Composite, ValueDef, Variant};

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

        let Some(thunk) = thunk else {
            return Ok(None);
        };

        let value = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;

        // Decode top-level fields.
        let multiaddr = named_field(&value, "multiaddr")
            .map(|v| String::from_utf8_lossy(&decode_byte_vec(v)).into_owned())
            .unwrap_or_default();

        let stake = named_field(&value, "stake")
            .and_then(|v| v.as_u128())
            .ok_or_else(|| ClientError::Chain("Missing 'stake'".to_string()))?;

        let committed_bytes = named_field(&value, "committed_bytes")
            .and_then(|v| v.as_u128())
            .ok_or_else(|| ClientError::Chain("Missing 'committed_bytes'".to_string()))?
            as u64;

        // Decode settings sub-composite.
        let settings = named_field(&value, "settings")
            .ok_or_else(|| ClientError::Chain("Missing 'settings' in ProviderInfo".to_string()))?;

        let replica_sync_price =
            named_field(settings, "replica_sync_price").and_then(|v| match &v.value {
                ValueDef::Variant(Variant { name, values }) if name == "Some" => {
                    values.values().next().and_then(|v| v.as_u128())
                }
                _ => None,
            });

        // Decode stats sub-composite.
        let stats = named_field(&value, "stats");
        let agreements_total = stats
            .and_then(|s| named_field(s, "agreements_total"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;
        let challenges_failed = stats
            .and_then(|s| named_field(s, "challenges_failed"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;

        Ok(Some(ProviderInfo {
            multiaddr,
            stake,
            committed_bytes,
            max_capacity: named_field(settings, "max_capacity")
                .and_then(|v| v.as_u128())
                .ok_or_else(|| ClientError::Chain("Missing 'max_capacity'".to_string()))?
                as u64,
            min_duration: named_field(settings, "min_duration")
                .and_then(|v| v.as_u128())
                .ok_or_else(|| ClientError::Chain("Missing 'min_duration'".to_string()))?
                as u32,
            max_duration: named_field(settings, "max_duration")
                .and_then(|v| v.as_u128())
                .ok_or_else(|| ClientError::Chain("Missing 'max_duration'".to_string()))?
                as u32,
            price_per_byte: named_field(settings, "price_per_byte")
                .and_then(|v| v.as_u128())
                .ok_or_else(|| ClientError::Chain("Missing 'price_per_byte'".to_string()))?,
            accepting_primary: named_field(settings, "accepting_primary")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| ClientError::Chain("Missing 'accepting_primary'".to_string()))?,
            replica_sync_price,
            accepting_extensions: named_field(settings, "accepting_extensions")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| ClientError::Chain("Missing 'accepting_extensions'".to_string()))?,
            agreements_total,
            challenges_failed,
            deregister_at: named_field(&value, "deregister_at").and_then(|v| match &v.value {
                ValueDef::Variant(Variant { name, values }) if name == "Some" => values
                    .values()
                    .next()
                    .and_then(|v| v.as_u128())
                    .map(|n| n as u32),
                _ => None,
            }),
        }))
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

        let Some(thunk) = thunk else {
            return Ok(None);
        };
        let decoded = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode replay state: {e}")))?;
        Ok(named_field(&decoded, "hsn")
            .and_then(|v| v.as_u128())
            .map(|h| h as u64))
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
            .map_err(|e| ClientError::Chain(format!("Failed to read RequestTimeout: {e}")))?
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode RequestTimeout: {e}")))?;

        Ok(value.as_u128().map(|v| v as u32))
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
        let provider_bytes: &[u8] = provider_account.as_ref();

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

        let mut agreements = Vec::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            // Key layout: [twox128(pallet)=16][twox128(storage)=16]
            //             [blake2_128(bucket_id)=16][bucket_id=8]
            //             [blake2_128(provider)=16][provider=32]
            // bucket_id at [48..56], provider at [72..104]
            let key = &kv.key_bytes;
            if key.len() < 104 {
                continue;
            }
            if &key[72..104] != provider_bytes {
                continue;
            }

            let bucket_id =
                u64::from_le_bytes(key[48..56].try_into().unwrap_or([0u8; 8])) as BucketId;

            let value = match kv.value.to_value() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to decode agreement: {e}");
                    continue;
                }
            };

            let owner = named_field(&value, "owner")
                .and_then(decode_account_bytes)
                .map(|b| format!("0x{}", hex::encode(b)))
                .unwrap_or_default();

            let max_bytes = named_field(&value, "max_bytes")
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u64;

            let expires_at = named_field(&value, "expires_at")
                .and_then(|v| v.as_u128())
                .unwrap_or(0) as u32;

            let is_primary = named_field(&value, "role")
                .map(|r| {
                    matches!(&r.value, ValueDef::Variant(Variant { name, .. }) if name == "Primary")
                })
                .unwrap_or(false);

            agreements.push(ActiveAgreement {
                bucket_id,
                owner,
                max_bytes,
                expires_at,
                is_primary,
            });
        }

        Ok(agreements)
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
        let provider_bytes: Vec<u8> = AsRef::<[u8]>::as_ref(&provider_account).to_vec();

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

            // Key layout: [twox128(pallet)=16][twox128(storage)=16]
            //             [blake2_128(deadline)=16][deadline=4]; deadline at [48..52]
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

            // Value is Vec<Challenge>, decoded as an unnamed Composite
            let ValueDef::Composite(Composite::Unnamed(items)) = &value.value else {
                continue;
            };
            let items: Vec<_> = items.iter().collect();

            for (index, item) in items.iter().enumerate() {
                let Some(pv) = named_field(item, "provider") else {
                    continue;
                };
                let Some(pv_bytes) = decode_account_bytes(pv) else {
                    continue;
                };
                if pv_bytes != provider_bytes {
                    continue;
                }

                let bucket_id = named_field(item, "bucket_id")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as BucketId;

                let leaf_index = named_field(item, "leaf_index")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as u64;

                let chunk_index = named_field(item, "chunk_index")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(0) as u64;

                challenges.push(ChallengeInfo {
                    challenge_id: (deadline, index as u16),
                    bucket_id,
                    deadline,
                    leaf_index,
                    chunk_index,
                });
            }
        }

        Ok(challenges)
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

        let Some(thunk) = thunk else {
            return Ok(ProviderStats::default());
        };

        let value = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;

        let stake = named_field(&value, "stake")
            .and_then(|v| v.as_u128())
            .unwrap_or(0);

        let committed_bytes = named_field(&value, "committed_bytes")
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u64;

        let stats = named_field(&value, "stats");

        let agreements_total = stats
            .and_then(|s| named_field(s, "agreements_total"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;

        let agreements_extended = stats
            .and_then(|s| named_field(s, "agreements_extended"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;

        let challenges_received = stats
            .and_then(|s| named_field(s, "challenges_received"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;

        let challenges_failed = stats
            .and_then(|s| named_field(s, "challenges_failed"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u32;

        let reputation = if agreements_total > 0 {
            let failure_rate = challenges_failed as f64 / agreements_total as f64;
            ((1.0 - failure_rate) * 100.0).clamp(0.0, 100.0) as u8
        } else {
            100
        };

        Ok(ProviderStats {
            stake,
            committed_bytes,
            agreements_total,
            agreements_extended,
            challenges_received,
            challenges_failed,
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

        let Some(thunk) = thunk else {
            return Ok(CapacityInfo {
                committed_bytes: 0,
                available_bytes: 0,
                stake: 0,
                required_stake: 0,
            });
        };

        let value = thunk
            .to_value()
            .map_err(|e| ClientError::Chain(format!("Failed to decode provider: {e}")))?;

        let stake = named_field(&value, "stake")
            .and_then(|v| v.as_u128())
            .unwrap_or(0);

        let committed_bytes = named_field(&value, "committed_bytes")
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u64;

        let settings = named_field(&value, "settings");

        let max_capacity = settings
            .and_then(|s| named_field(s, "max_capacity"))
            .and_then(|v| v.as_u128())
            .unwrap_or(0) as u64;

        let available_bytes = max_capacity.saturating_sub(committed_bytes);

        Ok(CapacityInfo {
            committed_bytes,
            available_bytes,
            stake,
            required_stake: 0,
        })
    }

    /// Monitor reputation score.
    pub async fn get_reputation(&self) -> ClientResult<u8> {
        let stats = self.get_stats().await?;
        Ok(stats.reputation)
    }
}

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

/// Decode an AccountId32 from a scale_value unnamed composite of 32 u8 items.
fn decode_account_bytes(value: &subxt::ext::scale_value::Value<u32>) -> Option<Vec<u8>> {
    match &value.value {
        ValueDef::Composite(Composite::Unnamed(items)) if items.len() == 32 => {
            items.iter().map(|b| b.as_u128().map(|n| n as u8)).collect()
        }
        _ => None,
    }
}

/// Decode a `Vec<u8>` / `BoundedVec<u8, _>` from a scale_value composite.
///
/// `BoundedVec<T, N>` serializes its `TypeInfo` as a 1-field unnamed composite
/// wrapping the inner `Vec<T>`, so scale_value surfaces it as
/// `Composite::Unnamed([inner_vec])`. This helper drills through that wrapper
/// if present, then collects the bytes.
fn decode_byte_vec(value: &subxt::ext::scale_value::Value<u32>) -> Vec<u8> {
    let ValueDef::Composite(Composite::Unnamed(items)) = &value.value else {
        return Vec::new();
    };
    // Direct sequence of byte primitives.
    let bytes: Vec<u8> = items
        .iter()
        .filter_map(|b| b.as_u128().map(|n| n as u8))
        .collect();
    if !items.is_empty() && bytes.len() == items.len() {
        return bytes;
    }
    // BoundedVec wrapper: single inner field holds the actual sequence.
    if items.len() == 1 {
        return decode_byte_vec(&items[0]);
    }
    Vec::new()
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
