//! Base client infrastructure for connecting to the storage system.
//!
//! Provides common functionality shared across all client types:
//! - Substrate connection management
//! - HTTP client for provider nodes
//! - Error handling
//! - Utility functions

use crate::substrate::SubstrateClient;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use subxt_signer::sr25519::Keypair;
use thiserror::Error;

/// Client errors.
#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Data verification failed")]
    VerificationFailed,

    #[error("Provider unavailable: {0}")]
    ProviderUnavailable(String),

    #[error("Chain error: {0}")]
    Chain(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Storage error: {0}")]
    Storage(String),
}

/// Configuration for connecting to the storage system.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// WebSocket URL for the substrate node (e.g., "ws://localhost:2222")
    pub chain_ws_url: String,
    /// Default provider node URLs
    pub provider_urls: Vec<String>,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Enable automatic retries
    pub enable_retries: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            chain_ws_url: "ws://localhost:2222".to_string(),
            provider_urls: vec!["http://localhost:3333".to_string()],
            timeout_secs: 30,
            enable_retries: true,
        }
    }
}

/// Base client for interacting with the storage system.
///
/// This provides common functionality for all specialized clients.
pub struct BaseClient {
    /// HTTP client for provider nodes
    pub(crate) http: HttpClient,
    /// Configuration
    pub(crate) config: ClientConfig,
    /// Substrate API client for on-chain operations
    pub(crate) chain_client: Option<Arc<SubstrateClient>>,
}

impl BaseClient {
    /// Create a new base client.
    ///
    /// Note: This does not connect to the chain. Use `connect_chain()` to establish
    /// a connection for on-chain operations.
    pub fn new(config: ClientConfig) -> Result<Self, ClientError> {
        let http = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| ClientError::Config(e.to_string()))?;

        Ok(Self {
            http,
            config,
            chain_client: None,
        })
    }

    /// Connect to the substrate chain.
    ///
    /// This must be called before any on-chain operations can be performed.
    pub async fn connect_chain(&mut self) -> Result<(), ClientError> {
        let client = SubstrateClient::connect(&self.config.chain_ws_url).await?;
        self.chain_client = Some(Arc::new(client));
        Ok(())
    }

    /// Get the chain client, returning an error if not connected.
    pub(crate) fn chain(&self) -> Result<&SubstrateClient, ClientError> {
        self.chain_client
            .as_ref()
            .map(|c| c.as_ref())
            .ok_or_else(|| {
                ClientError::Config(
                    "Not connected to chain. Call connect_chain() first.".to_string(),
                )
            })
    }

    /// Set a signer for submitting extrinsics (builder pattern).
    pub fn with_dev_signer(mut self, name: &str) -> Result<Self, ClientError> {
        self.set_dev_signer(name)?;
        Ok(self)
    }

    /// Set a dev signer for submitting extrinsics (mutable reference).
    pub fn set_dev_signer(&mut self, name: &str) -> Result<(), ClientError> {
        if let Some(client) = self.chain_client.as_mut() {
            let client = Arc::make_mut(client);
            *client = client.clone().with_dev_signer(name)?;
            Ok(())
        } else {
            Err(ClientError::Config(
                "Must connect to chain before setting signer".to_string(),
            ))
        }
    }

    /// Set a custom keypair signer for submitting extrinsics.
    ///
    /// Must be called after `connect_chain()`.
    pub fn set_signer(&mut self, signer: Keypair) -> Result<(), ClientError> {
        if let Some(client) = self.chain_client.as_mut() {
            let client = Arc::make_mut(client);
            *client = client.clone().with_signer(signer);
            Ok(())
        } else {
            Err(ClientError::Config(
                "Must connect to chain before setting signer".to_string(),
            ))
        }
    }

    /// Get a provider URL (round-robin or based on strategy).
    pub(crate) fn get_provider_url(&self) -> Result<&str, ClientError> {
        self.config
            .provider_urls
            .first()
            .map(|s| s.as_str())
            .ok_or_else(|| ClientError::Config("No provider URLs configured".to_string()))
    }

    /// Helper to decode hex strings.
    pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, ClientError> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        hex::decode(s).map_err(|e| ClientError::Serialization(format!("Invalid hex: {e}")))
    }

    /// Helper to encode to hex strings.
    pub(crate) fn hex_encode(data: &[u8]) -> String {
        format!("0x{}", hex::encode(data))
    }
}

// Common API types used across clients

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub multiaddr: String,
    pub stake: u128,
    pub committed_bytes: u64,
    pub price_per_byte: u128,
    pub accepting_primary: bool,
    pub reputation: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketInfo {
    pub bucket_id: u64,
    pub min_providers: u32,
    pub frozen: bool,
    pub snapshot: Option<SnapshotInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub checkpoint_block: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgreementInfo {
    pub provider: String,
    pub max_bytes: u64,
    pub payment_locked: u128,
    pub expires_at: u32,
    pub is_primary: bool,
}

/// Chunking strategy for data upload.
#[derive(Debug, Clone, Copy)]
pub enum ChunkingStrategy {
    /// Fixed-size chunks (default: 256 KiB)
    Fixed(usize),
    /// Content-defined chunking (not yet implemented)
    ContentDefined,
}

impl Default for ChunkingStrategy {
    fn default() -> Self {
        Self::Fixed(256 * 1024)
    }
}

/// Result type for client operations.
pub type ClientResult<T> = Result<T, ClientError>;
