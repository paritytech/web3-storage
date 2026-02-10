//! Discovery Client - For finding and matching storage providers.
//!
//! This client provides operations for:
//! - Finding providers that match storage requirements
//! - Querying provider capacity and availability
//! - Getting recommendations for provider selection

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};

/// Storage requirements for provider matching.
#[derive(Debug, Clone)]
pub struct StorageRequirements {
    /// Bytes needed for storage.
    pub bytes_needed: u64,
    /// Minimum agreement duration in blocks.
    pub min_duration: u32,
    /// Maximum acceptable price per byte.
    pub max_price_per_byte: u128,
    /// If true, only match providers accepting primary agreements.
    pub primary_only: bool,
}

impl Default for StorageRequirements {
    fn default() -> Self {
        Self {
            bytes_needed: 0,
            min_duration: 0,
            max_price_per_byte: u128::MAX,
            primary_only: true,
        }
    }
}

/// Reason for partial match when provider doesn't fully meet requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartialMatchReason {
    /// Provider's price exceeds max_price_per_byte.
    PriceTooHigh,
    /// Provider doesn't have enough available capacity.
    InsufficientCapacity,
    /// Provider's duration constraints don't match.
    DurationMismatch,
    /// Provider is not accepting agreements.
    NotAccepting,
}

/// Provider matching result.
#[derive(Debug, Clone)]
pub struct MatchedProvider {
    /// Provider account ID (SS58 encoded).
    pub account: String,
    /// Provider information.
    pub info: ProviderInfo,
    /// Match score (0-100, 100 = perfect match).
    pub match_score: u8,
    /// Available capacity in bytes (None if unlimited).
    pub available_capacity: Option<u64>,
    /// If not a perfect match, why.
    pub partial_reason: Option<PartialMatchReason>,
}

/// Provider information for discovery.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// Network address for connecting.
    pub multiaddr: String,
    /// Total stake locked.
    pub stake: u128,
    /// Currently committed bytes.
    pub committed_bytes: u64,
    /// Maximum capacity (0 = unlimited).
    pub max_capacity: u64,
    /// Minimum agreement duration.
    pub min_duration: u32,
    /// Maximum agreement duration.
    pub max_duration: u32,
    /// Price per byte per block.
    pub price_per_byte: u128,
    /// Whether accepting primary agreements.
    pub accepting_primary: bool,
    /// Replica sync price (None if not accepting replicas).
    pub replica_sync_price: Option<u128>,
    /// Whether accepting extensions.
    pub accepting_extensions: bool,
    /// Total agreements ever.
    pub agreements_total: u32,
    /// Failed challenges count.
    pub challenges_failed: u32,
}

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
    ///
    /// # Parameters
    /// - `requirements`: Storage requirements to match against
    /// - `limit`: Maximum number of providers to return
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::discovery::{DiscoveryClient, StorageRequirements};
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
    ///     println!("Provider {}: score {}", provider.account, provider.match_score);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn find_providers(
        &self,
        requirements: StorageRequirements,
        limit: u32,
    ) -> ClientResult<Vec<MatchedProvider>> {
        let chain = self.base.chain()?;

        // Call runtime API via storage query
        // For now, we'll iterate providers using storage queries
        // In production, this would use the runtime API directly
        tracing::info!(
            "Finding providers: {} bytes, {} duration, max price {}",
            requirements.bytes_needed,
            requirements.min_duration,
            requirements.max_price_per_byte
        );

        // TODO: Implement actual runtime API call
        // For now, return empty vec - implementation would use:
        // chain.api().runtime_api().find_matching_providers(requirements, limit)

        Ok(vec![])
    }

    /// Find the best provider for the given requirements.
    ///
    /// Returns the highest-scoring provider, or None if no providers match.
    pub async fn find_best_provider(
        &self,
        requirements: StorageRequirements,
    ) -> ClientResult<Option<MatchedProvider>> {
        let providers = self.find_providers(requirements, 1).await?;
        Ok(providers.into_iter().next())
    }

    /// Get providers with sufficient capacity for the given bytes.
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

        // TODO: Implement actual runtime API call
        Ok(vec![])
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

        let requirements = StorageRequirements {
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
                let estimated_cost = provider.info.price_per_byte
                    * bytes as u128
                    * duration as u128;

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
        let _chain = self.base.chain()?;

        tracing::info!("Getting provider info for {}", account);

        // TODO: Implement actual runtime API call
        Ok(None)
    }

    /// Check if a provider can accept additional bytes.
    pub async fn can_provider_accept(&self, account: &str, additional_bytes: u64) -> ClientResult<bool> {
        let _chain = self.base.chain()?;

        tracing::info!(
            "Checking if provider {} can accept {} bytes",
            account,
            additional_bytes
        );

        // TODO: Implement actual runtime API call
        Ok(false)
    }

    /// List all registered providers (paginated).
    pub async fn list_providers(
        &self,
        offset: u32,
        limit: u32,
    ) -> ClientResult<Vec<(String, ProviderInfo)>> {
        let _chain = self.base.chain()?;

        tracing::info!("Listing providers (offset={}, limit={})", offset, limit);

        // TODO: Implement actual runtime API call
        Ok(vec![])
    }
}
