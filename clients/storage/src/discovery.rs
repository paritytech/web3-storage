// SPDX-License-Identifier: Apache-2.0

//! Discovery Client - For finding and matching storage providers.
//!
//! This client provides operations for:
//! - Finding providers that match storage requirements
//! - Querying provider capacity and availability
//! - Getting recommendations for provider selection

use crate::base::{BaseClient, ClientConfig, ClientError, ClientResult};
use crate::convert;
use crate::substrate::SubstrateClient;
use sp_core::crypto::Ss58Codec;
use storage_subxt::api;
use storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api as rt_api;

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderInfo {
    /// Network address for connecting.
    pub multiaddr: String,
    /// Raw registered public key bytes; the scheme of the provider's
    /// signatures is whatever this key belongs to.
    pub public_key: Vec<u8>,
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
    /// Block at which deregistration becomes finalisable (`None` = not deregistering).
    pub deregister_at: Option<u32>,
    /// Reputation 0-100, computed on-chain by `runtime_api::reputation_score`.
    pub reputation: u8,
}

impl From<rt_api::ProviderInfoResponse> for ProviderInfo {
    fn from(p: rt_api::ProviderInfoResponse) -> Self {
        Self {
            multiaddr: String::from_utf8_lossy(&p.multiaddr).into_owned(),
            public_key: p.public_key,
            stake: p.stake,
            committed_bytes: p.committed_bytes,
            max_capacity: p.max_capacity,
            min_duration: p.min_duration,
            max_duration: p.max_duration,
            price_per_byte: p.price_per_byte,
            accepting_primary: p.accepting_primary,
            replica_sync_price: p.replica_sync_price,
            accepting_extensions: p.accepting_extensions,
            agreements_total: p.agreements_total,
            challenges_failed: p.challenges_failed,
            deregister_at: p.deregister_at,
            reputation: p.reputation,
        }
    }
}

impl From<rt_api::PartialMatchReason> for PartialMatchReason {
    fn from(r: rt_api::PartialMatchReason) -> Self {
        match r {
            rt_api::PartialMatchReason::PriceTooHigh => Self::PriceTooHigh,
            rt_api::PartialMatchReason::InsufficientCapacity => Self::InsufficientCapacity,
            rt_api::PartialMatchReason::DurationMismatch => Self::DurationMismatch,
            rt_api::PartialMatchReason::NotAccepting => Self::NotAccepting,
        }
    }
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
    /// Ties are broken by price ascending (cheaper wins).
    ///
    /// Matching and scoring happen on-chain via
    /// `StorageProviderApi::find_matching_providers`, so this is one call
    /// rather than a scan of the whole provider directory.
    ///
    /// # Results are ranked, not filtered
    ///
    /// `max_price_per_byte` is a *scoring* input, not a hard filter: a
    /// provider charging above it is still returned, with its score reduced
    /// and [`PartialMatchReason::PriceTooHigh`] set. Callers that need a
    /// budget guarantee must check `info.price_per_byte` themselves. The same
    /// is true of capacity and duration. Providers that have announced
    /// deregistration are excluded outright.
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
        tracing::info!(
            "Finding providers: {} bytes, {} duration, max price {}",
            requirements.bytes_needed,
            requirements.min_duration,
            requirements.max_price_per_byte
        );

        let chain = self.base.chain()?;
        let at = chain.at_current_block().await?;

        let matched = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .find_matching_providers(
                        rt_api::StorageRequirements {
                            bytes_needed: requirements.bytes_needed,
                            min_duration: requirements.min_duration,
                            max_price_per_byte: requirements.max_price_per_byte,
                            primary_only: requirements.primary_only,
                        },
                        limit,
                    ),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("find_matching_providers runtime API failed: {e}"))
            })?;

        // Already sorted by score descending, price ascending, and truncated
        // to `limit` on-chain.
        Ok(matched
            .into_iter()
            .filter_map(|m| {
                let account = convert::account_from_runtime_api(&m.account, "provider")?;
                Some(MatchedProvider {
                    account: convert::to_sp_account(&account).to_ss58check(),
                    info: ProviderInfo::from(m.info),
                    match_score: m.match_score,
                    available_capacity: m.available_capacity,
                    partial_reason: m.partial_reason.map(PartialMatchReason::from),
                })
            })
            .collect())
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

    /// Get providers with sufficient capacity for the given bytes (paginated).
    ///
    /// Only returns providers that are accepting agreements, have enough
    /// available capacity, and hold stake sufficient to back the extra bytes.
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
        tracing::info!(
            "Finding providers with {} bytes capacity (offset={}, limit={})",
            bytes_needed,
            offset,
            limit
        );

        let chain = self.base.chain()?;
        let at = chain.at_current_block().await?;

        let page = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .providers_with_capacity(bytes_needed, offset, limit),
            )
            .await
            .map_err(|e| {
                ClientError::Chain(format!("providers_with_capacity runtime API failed: {e}"))
            })?;

        Ok(page
            .into_iter()
            .map(|(account, info)| {
                (
                    convert::to_sp_account(&account).to_ss58check(),
                    ProviderInfo::from(info),
                )
            })
            .collect())
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
            .filter_map(|provider| {
                // Calculate estimated cost
                let estimated_cost = provider
                    .info
                    .price_per_byte
                    .saturating_mul(bytes as u128)
                    .saturating_mul(duration as u128);

                // filter out providers whose price above user's budget
                if estimated_cost > budget {
                    return None;
                }

                // Reputation is defined once, on-chain, by
                // `runtime_api::reputation_score` - never recomputed here.
                let reliability_score = provider.info.reputation;

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

                Some(ProviderRecommendation {
                    provider,
                    estimated_cost,
                    reliability_score,
                    reason,
                })
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

        let at = chain.at_current_block().await?;
        let info = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .provider_info(convert::to_subxt_account(&account_id)),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("provider_info runtime API failed: {e}")))?;

        Ok(info.map(ProviderInfo::from))
    }

    /// Check if a provider can accept additional bytes.
    ///
    /// Returns true if the provider is accepting agreements, has sufficient
    /// available capacity, and holds stake covering the extra bytes.
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

        let at = chain.at_current_block().await?;
        at.runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .can_accept_bytes(convert::to_subxt_account(&account_id), additional_bytes),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("can_accept_bytes runtime API failed: {e}")))
    }

    /// List all registered providers (paginated).
    pub async fn list_providers(
        &self,
        offset: u32,
        limit: u32,
    ) -> ClientResult<Vec<(String, ProviderInfo)>> {
        tracing::info!("Listing providers (offset={}, limit={})", offset, limit);

        let chain = self.base.chain()?;
        let at = chain.at_current_block().await?;

        let page = at
            .runtime_apis()
            .call(
                api::runtime_apis()
                    .storage_provider_api()
                    .providers(offset, limit),
            )
            .await
            .map_err(|e| ClientError::Chain(format!("providers runtime API failed: {e}")))?;

        Ok(page
            .into_iter()
            .map(|(account, info)| {
                (
                    convert::to_sp_account(&account).to_ss58check(),
                    ProviderInfo::from(info),
                )
            })
            .collect())
    }
}
