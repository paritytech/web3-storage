// SPDX-License-Identifier: Apache-2.0

//! Discovery Client - For finding and matching storage providers.
//!
//! This client provides operations for:
//! - Finding providers that match storage requirements
//! - Querying provider capacity and availability
//! - Getting recommendations for provider selection

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::substrate::{storage, SubstrateClient};
use rt::pallet_storage_provider::pallet::ProviderInfo;
use rt_api::MatchedProvider;
use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use storage_subxt::api as runtime;
use storage_subxt::api::runtime_types as rt;
use storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api as rt_api;

/// Provider recommendation with additional context.
#[derive(Debug, Clone)]
pub struct ProviderRecommendation {
    /// The matched provider.
    pub provider: MatchedProvider,
    /// Estimated cost for the storage requirements.
    pub estimated_cost: u128,
    /// Reliability score based on challenge history (0-100).
    pub reliability_score: u8,
    /// Recommendation reason.
    pub reason: String,
}

/// Client for provider discovery and matching.
pub struct DiscoveryClient {
    base: BaseClient,
}

impl DiscoveryClient {
    /// Create a new discovery client.
    pub fn new(config: ClientConfig) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
        })
    }

    /// Create with default configuration.
    pub fn with_defaults() -> ClientResult<Self> {
        Self::new(ClientConfig::default())
    }

    /// Connect to the blockchain. Must be called before any on-chain operations.
    pub async fn connect(&mut self) -> ClientResult<()> {
        self.base.connect_chain().await
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Provider Discovery
    // ═════════════════════════════════════════════════════════════════════════

    /// Find providers matching the given storage requirements.
    ///
    /// Returns up to `limit` providers, sorted by match score (best first).
    /// Ties are broken by price ascending (cheaper wins).
    ///
    /// # Parameters
    /// - `requirements`: Storage requirements to match against
    /// - `limit`: Maximum number of providers to return
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::discovery::DiscoveryClient;
    /// # use storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api::StorageRequirements;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = DiscoveryClient::with_defaults()?;
    /// client.connect().await?;
    ///
    /// let requirements = StorageRequirements {
    ///     bytes_needed: 10 * 1024 * 1024 * 1024, // 10 GB
    ///     min_duration: 100_000,
    ///     max_price_per_byte: 1_000_000,
    ///     primary_only: true,
    /// };
    ///
    /// let providers = client.find_providers(requirements, 10).await?;
    /// for provider in providers {
    ///     println!("Provider {:?}: score {}", provider.account, provider.match_score);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn find_providers(
        &self,
        requirements: rt_api::StorageRequirements,
        limit: u32,
    ) -> ClientResult<Vec<MatchedProvider>> {
        let chain = self.base.chain()?;

        tracing::info!(
            "Finding providers: {} bytes, {} duration, max price {}",
            requirements.bytes_needed,
            requirements.min_duration,
            requirements.max_price_per_byte
        );

        let raw = chain
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("runtime api: {e}")))?
            .call(
                runtime::apis()
                    .storage_provider_api()
                    .find_matching_providers(requirements, limit),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("find_matching_providers: {e}")))?;

        Ok(raw)
    }

    /// Find the best provider for the given requirements.
    ///
    /// Returns the highest-scoring provider, or None if no providers match.
    pub async fn find_best_provider(
        &self,
        requirements: rt_api::StorageRequirements,
    ) -> ClientResult<Option<MatchedProvider>> {
        let providers = self.find_providers(requirements, 1).await?;
        Ok(providers.into_iter().next())
    }

    /// Get providers with sufficient capacity for the given bytes (paginated).
    ///
    /// Only returns providers that are accepting agreements and have enough
    /// available capacity.
    ///
    /// # Parameters
    /// - `bytes_needed`: Storage capacity needed
    /// - `offset`: Pagination offset
    /// - `limit`: Maximum number of providers to return
    pub async fn providers_with_capacity(
        &self,
        bytes_needed: u64,
        offset: u32,
        limit: u32,
    ) -> ClientResult<Vec<(String, ProviderInfo)>> {
        let chain = self.base.chain()?;

        tracing::info!(
            "Finding providers with {} bytes capacity (offset={}, limit={})",
            bytes_needed,
            offset,
            limit
        );

        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let mut iter = storage
            .iter(storage::all_providers())
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate providers: {e}")))?;

        let mut matching: Vec<(String, ProviderInfo)> = Vec::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            let account_str = match account_ss58_from_key(&kv.key_bytes) {
                Some(s) => s,
                None => continue,
            };

            let info = kv.value;

            // Must be accepting some kind of agreement
            if !info.settings.accepting_primary && info.settings.replica_sync_price.is_none() {
                continue;
            }

            // Must have sufficient available capacity (0 = unlimited)
            if info.settings.max_capacity > 0 {
                let available = info
                    .settings
                    .max_capacity
                    .saturating_sub(info.committed_bytes);
                if available < bytes_needed {
                    continue;
                }
            }

            matching.push((account_str, info));
        }

        let page: Vec<(String, ProviderInfo)> = matching
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();

        Ok(page)
    }

    /// Get recommendations for provider selection based on requirements and budget.
    ///
    /// This method provides intelligent recommendations that consider:
    /// - Price vs. quality tradeoffs
    /// - Provider reliability (challenge history)
    /// - Geographic distribution (if multiaddr parsing is available)
    ///
    /// # Parameters
    /// - `bytes`: Storage capacity needed
    /// - `duration`: Agreement duration in blocks
    /// - `budget`: Maximum total payment
    pub async fn suggest_providers(
        &self,
        bytes: u64,
        duration: u32,
        budget: u128,
    ) -> ClientResult<Vec<ProviderRecommendation>> {
        // Calculate max price per byte from budget
        let max_price_per_byte = if bytes > 0 && duration > 0 {
            budget / (bytes as u128 * duration as u128)
        } else {
            u128::MAX
        };

        let requirements = rt_api::StorageRequirements {
            bytes_needed: bytes,
            min_duration: duration,
            max_price_per_byte,
            primary_only: true,
        };

        let providers = self.find_providers(requirements, 10).await?;

        // Score and rank providers
        let recommendations: Vec<ProviderRecommendation> = providers
            .into_iter()
            .map(|provider| {
                // Calculate estimated cost
                let estimated_cost =
                    provider.info.price_per_byte * bytes as u128 * duration as u128;

                // Calculate reliability score based on challenge history
                let reliability_score = if provider.info.agreements_total > 0 {
                    let failure_rate = provider.info.challenges_failed as f64
                        / provider.info.agreements_total as f64;
                    ((1.0 - failure_rate) * 100.0) as u8
                } else {
                    50 // Neutral score for new providers
                };

                // Generate recommendation reason
                let reason = if provider.match_score == 100 && reliability_score >= 90 {
                    "Excellent match with high reliability".to_string()
                } else if provider.match_score >= 80 {
                    "Good match for your requirements".to_string()
                } else if estimated_cost < budget / 2 {
                    "Budget-friendly option".to_string()
                } else {
                    "Partial match - consider alternatives".to_string()
                };

                ProviderRecommendation {
                    provider,
                    estimated_cost,
                    reliability_score,
                    reason,
                }
            })
            .collect();

        Ok(recommendations)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Provider Information
    // ═════════════════════════════════════════════════════════════════════════

    /// Get detailed information about a specific provider.
    pub async fn get_provider_info(&self, account: &str) -> ClientResult<Option<ProviderInfo>> {
        let chain = self.base.chain()?;

        tracing::info!("Getting provider info for {}", account);

        let account_id = SubstrateClient::parse_account(account)?;

        chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::provider_info(&account_id))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))
    }

    /// Check if a provider can accept additional bytes.
    ///
    /// Returns true if the provider is accepting agreements and has sufficient
    /// available capacity. Does not verify stake sufficiency (requires a runtime
    /// constant not available via storage queries).
    pub async fn can_provider_accept(
        &self,
        account: &str,
        additional_bytes: u64,
    ) -> ClientResult<bool> {
        let chain = self.base.chain()?;

        tracing::info!(
            "Checking if provider {} can accept {} bytes",
            account,
            additional_bytes
        );

        let account_id = SubstrateClient::parse_account(account)?;

        let Some(info) = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?
            .fetch(&storage::provider_info(&account_id))
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to fetch provider: {e}")))?
        else {
            return Ok(false);
        };

        if !info.settings.accepting_primary && info.settings.replica_sync_price.is_none() {
            return Ok(false);
        }

        if info.settings.max_capacity > 0 {
            let available = info
                .settings
                .max_capacity
                .saturating_sub(info.committed_bytes);
            return Ok(available >= additional_bytes);
        }

        Ok(true) // Unlimited capacity
    }

    /// List all registered providers (paginated).
    pub async fn list_providers(
        &self,
        offset: u32,
        limit: u32,
    ) -> ClientResult<Vec<(String, ProviderInfo)>> {
        let chain = self.base.chain()?;

        tracing::info!("Listing providers (offset={}, limit={})", offset, limit);

        let storage = chain
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to get storage: {e}")))?;

        let mut iter = storage
            .iter(storage::all_providers())
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to iterate providers: {e}")))?;

        let mut all: Vec<(String, ProviderInfo)> = Vec::new();

        while let Some(result) = iter.next().await {
            let kv =
                result.map_err(|e| ClientError::Chain(format!("Storage iteration error: {e}")))?;

            let account_str = match account_ss58_from_key(&kv.key_bytes) {
                Some(s) => s,
                None => continue,
            };

            all.push((account_str, kv.value));
        }

        let page: Vec<(String, ProviderInfo)> = all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();

        Ok(page)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the provider's `AccountId32` from a `Providers` storage key and render it as
/// SS58.
///
/// Key layout: `[twox128(pallet)=16][twox128(storage)=16][blake2_128(account)=16][account=32]`,
/// so the raw account bytes live at `[48..80]`. Returns `None` when the key is too short
/// to contain the account suffix.
fn account_ss58_from_key(key: &[u8]) -> Option<String> {
    if key.len() < 80 {
        return None;
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&key[48..80]);
    Some(AccountId32::from(bytes).to_ss58check())
}
