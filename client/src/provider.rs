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
use crate::substrate::{extrinsics, storage};
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

    /// Query a provider's current settings from the chain.
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
            .map(|v| match &v.value {
                ValueDef::Composite(Composite::Unnamed(items)) => {
                    let bytes: Vec<u8> = items
                        .iter()
                        .filter_map(|b| b.as_u128().map(|n| n as u8))
                        .collect();
                    String::from_utf8_lossy(&bytes).into_owned()
                }
                _ => String::new(),
            })
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
        // TODO: Submit extrinsic
        tracing::info!("Would add stake: {}", additional_stake);
        Ok(())
    }

    /// Deregister as a provider (requires no active agreements).
    pub async fn deregister(&self) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!("Would deregister provider {}", self.provider_account);
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Agreement Management
    // ═════════════════════════════════════════════════════════════════════════

    /// Accept a storage agreement request for a bucket.
    ///
    /// This commits you to storing data for the specified bucket.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::ProviderClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ProviderClient::with_defaults("5GrwvaEF...".to_string())?;
    /// client.accept_agreement(1).await?;
    /// println!("Agreement accepted!");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn accept_agreement(&self, bucket_id: BucketId) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Accepting agreement for bucket {} as provider {}",
            bucket_id,
            self.provider_account
        );

        // Create and submit the extrinsic
        let tx = extrinsics::accept_agreement(bucket_id);

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

        tracing::info!("Agreement accepted successfully");
        Ok(())
    }

    /// List all pending agreement requests for this provider.
    pub async fn list_pending_requests(&self) -> ClientResult<Vec<AgreementRequest>> {
        // TODO: Query chain storage
        Ok(vec![])
    }

    /// List all active agreements for this provider.
    pub async fn list_active_agreements(&self) -> ClientResult<Vec<ActiveAgreement>> {
        // TODO: Query chain storage via Runtime API
        Ok(vec![])
    }

    /// Confirm replica sync to receive payment.
    ///
    /// For replica providers, this confirms you've synced data and
    /// triggers payment from the sync_balance.
    pub async fn confirm_replica_sync(
        &self,
        bucket_id: BucketId,
        mmr_roots: [Option<H256>; 7],
        _signature: Vec<u8>,
    ) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!(
            "Would confirm replica sync for bucket {} with {} roots",
            bucket_id,
            mmr_roots.iter().filter(|r| r.is_some()).count()
        );
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
        // TODO: Query chain storage
        Ok(vec![])
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Monitoring & Analytics
    // ═════════════════════════════════════════════════════════════════════════

    /// Get your provider statistics.
    pub async fn get_stats(&self) -> ClientResult<ProviderStats> {
        // TODO: Query via Runtime API
        Ok(ProviderStats::default())
    }

    /// Get your total earnings (all time).
    pub async fn get_total_earnings(&self) -> ClientResult<u128> {
        // Calculate from finalized agreements
        Ok(0)
    }

    /// Get your current committed bytes vs available capacity.
    pub async fn get_capacity_info(&self) -> ClientResult<CapacityInfo> {
        // TODO: Query chain storage
        Ok(CapacityInfo {
            committed_bytes: 0,
            available_bytes: 0,
            stake: 0,
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
