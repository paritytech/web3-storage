//! Provider Client - For storage providers managing their operations.
//!
//! This client provides operations for:
//! - Registering as a provider
//! - Managing provider settings (pricing, availability)
//! - Accepting storage agreements
//! - Responding to challenges
//! - Monitoring earnings and performance

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::substrate::{extrinsics, SubstrateClient};
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_primitives::BucketId;
use subxt::tx::TxProgress;

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

    // ═════════════════════════════════════════════════════════════════════════
    // Provider Registration & Settings
    // ═════════════════════════════════════════════════════════════════════════

    /// Register as a storage provider on-chain.
    ///
    /// This creates a provider profile with initial settings.
    ///
    /// # Parameters
    /// - `multiaddr`: Network address for clients to connect (e.g., "/ip4/1.2.3.4/tcp/3000")
    /// - `public_key`: Public key for signature verification (32-64 bytes)
    /// - `stake`: Initial stake to lock (in smallest unit)
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::ProviderClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ProviderClient::with_defaults("5GrwvaEF...".to_string())?;
    /// let multiaddr = "/ip4/203.0.113.1/tcp/3000".to_string();
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
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {}", e)))?;

        // Wait for finalization
        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {}", e)))?;

        tracing::info!("Provider registered successfully");
        Ok(())
    }

    /// Update provider settings.
    ///
    /// Change pricing, availability, or other settings.
    pub async fn update_settings(&self, settings: ProviderSettings) -> ClientResult<()> {
        // TODO: Submit extrinsic
        tracing::info!("Would update settings: {:?}", settings);
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
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {}", e)))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {}", e)))?;

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
        signature: Vec<u8>,
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
    /// # Example
    /// ```no_run
    /// # use storage_client::ProviderClient;
    /// # async fn example(challenge_id: (u32, u16)) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ProviderClient::with_defaults("5GrwvaEF...".to_string())?;
    ///
    /// // Fetch data and generate proofs from local storage
    /// let chunk_data = vec![0u8; 256 * 1024];
    /// let chunk_proof = vec![]; // Merkle proof
    /// let mmr_proof = todo!(); // MMR proof
    ///
    /// client.respond_to_challenge(
    ///     challenge_id,
    ///     chunk_data,
    ///     chunk_proof,
    ///     mmr_proof
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn respond_to_challenge(
        &self,
        bucket_id: BucketId,
        challenge_id: (u32, u16), // (deadline, index)
        chunk_data: Vec<u8>,
        chunk_proof: Vec<H256>,
        mmr_proof: MmrProofData,
    ) -> ClientResult<()> {
        let chain = self.base.chain()?;
        let signer = chain.signer()?;

        tracing::info!(
            "Responding to challenge {:?} with {} bytes",
            challenge_id,
            chunk_data.len()
        );

        // Create and submit the extrinsic
        let tx = extrinsics::respond_challenge(
            bucket_id,
            challenge_id,
            chunk_data,
            chunk_proof,
            (mmr_proof.peaks, mmr_proof.siblings),
        );

        let tx_progress = chain
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to submit tx: {}", e)))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| ClientError::Chain(format!("Transaction failed: {}", e)))?;

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

// Types

#[derive(Debug, Clone)]
pub struct ProviderSettings {
    pub price_per_byte: u128,
    pub min_duration: u32,
    pub max_duration: u32,
    pub accepting_primary: bool,
    pub replica_sync_price: Option<u128>,
    pub accepting_extensions: bool,
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

#[derive(Debug, Clone)]
pub struct MmrProofData {
    pub peaks: Vec<H256>,
    pub siblings: Vec<H256>,
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
