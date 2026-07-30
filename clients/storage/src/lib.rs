// SPDX-License-Identifier: Apache-2.0

//! # Storage Client SDK
//!
//! Comprehensive off-chain SDK for interacting with the scalable Web3 storage system.
//!
//! ## Architecture
//!
//! The SDK provides specialized clients for different user roles:
//!
//! ### For Storage Users
//! [`StorageUserClient`] - Upload, download, and verify data
//! ```no_run
//! use storage_client::{StorageUserClient, ClientConfig, ChunkingStrategy, Signer};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
//!
//! // Upload data
//! let data = b"Hello, decentralized world!";
//! let data_root = client.upload(1, data, ChunkingStrategy::default()).await?;
//!
//! // Commit to chain
//! let commitment = client.commit(1, vec![data_root], 0u64).await?;
//!
//! // Download and verify
//! let retrieved = client.download(&data_root, 0, data.len() as u64).await?;
//! assert_eq!(retrieved, data);
//! # Ok(())
//! # }
//! ```
//!
//! ### For Storage Providers
//! [`ProviderClient`] - Manage provider operations
//! ```no_run
//! use storage_client::{ProviderClient, Signer};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = ProviderClient::with_defaults(Signer::from_seed("//Alice")?)?;
//!
//! // Register as provider
//! client.register(
//!     "/ip4/1.2.3.4/tcp/3333".to_string(),
//!     vec![0u8; 32], // public key
//!     1_000_000_000_000, // stake
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### For Bucket Administrators
//! [`AdminClient`] - Manage buckets and agreements
//! ```no_run
//! use storage_client::{AdminClient, NegotiateRequest, ProviderClient, Signer};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = AdminClient::with_defaults(Signer::from_seed("//Alice")?)?;
//!
//! // 1. Ask the provider node to sign agreement terms over HTTP. The
//! //    provider allocates the nonce + validity window and signs.
//! let signed = ProviderClient::negotiate_terms(
//!     "http://provider.example:3333",
//!     &NegotiateRequest {
//!         owner: "5GrwvaEF...".parse()?,
//!         max_bytes: 10 * 1024 * 1024 * 1024, // 10 GB
//!         duration: 100_000,
//!         price_per_byte: 1_000_000,
//!         replica_params: None,
//!         bucket_id: None,
//!     },
//! ).await?;
//!
//! // 2. Redeem them on-chain — bucket creation + primary agreement
//! //    happen atomically inside `establish_storage_agreement`.
//! let bucket_id = client.establish_storage_agreement(
//!     "5FHneW46...".to_string(), // provider account
//!     signed,
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### For Data Integrity Monitors
//! [`ChallengerClient`] - Challenge providers
//! ```no_run
//! use storage_client::{ChallengerClient, ChunkLocation, Signer};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = ChallengerClient::with_defaults(Signer::from_seed("//Alice")?)?;
//!
//! // Challenge a provider
//! let challenge_id = client.challenge_checkpoint(
//!     1, // bucket_id
//!     "5FHneW46...".to_string(), // provider
//!     ChunkLocation { leaf_index: 5, chunk_index: 123 },
//! ).await?;
//! # Ok(())
//! # }
//! ```

// Re-export main types
pub mod admin;
pub mod base;
pub mod challenger;
pub mod checkpoint;
pub mod checkpoint_persistence;
pub mod discovery;
pub mod encryption;
pub mod provider;
pub mod signer;
pub mod storage_user;
pub mod substrate;
pub mod verification;

/// Negotiation wire types, re-exported from `provider-negotiation`.
pub use provider_negotiation as agreement;

/// Typed block / event subscription over the chain, re-exported from
/// `storage-indexers`.
///
/// The stream constructors deliberately return [`IndexerError`] rather than
/// wrapping it in [`ClientError`](base::ClientError): the streams are a
/// standalone building block usable without the rest of this SDK, and their
/// failure modes (transport, connect, subscribe) are its whole error surface.
pub use storage_indexers::{
    BlockEvent, BlockStream, EventFilter, EventStream, IndexerError, DRIVE_REGISTRY_PALLET,
    S3_REGISTRY_PALLET, STORAGE_PALLETS, STORAGE_PROVIDER_PALLET,
};

// Re-export commonly used types
pub use admin::AdminClient;
pub use agreement::{sign_terms, AgreementTermsOf, NegotiateRequest, ReplicaTermsOf, SignedTerms};
pub use base::{ChunkingStrategy, ClientConfig, ClientError, ClientResult};
pub use challenger::ChallengerClient;
pub use checkpoint::{
    AutoChallengeConfig, AutoChallengeResult, BatchedCheckpointConfig, BatchedInterval,
    BucketCheckpointStatus, ChallengeEvidence, ChallengeReason, ChallengeRecommendation,
    CheckpointCallback, CheckpointConfig, CheckpointLoopCommand, CheckpointLoopHandle,
    CheckpointManager, CheckpointMetrics, CheckpointResult, CommitmentCollection,
    ConflictResolution, ConflictType, ConflictingProvider, FailedChallenge, ProviderConflict,
    ProviderHealthHistory, ProviderInfo, ProviderStatus, SubmittedChallenge,
};
pub use checkpoint_persistence::{
    CheckpointPersistence, PersistedBucketStatus, PersistedCheckpointState, PersistedConflict,
    PersistedHealthHistory, PersistedMetrics, PersistenceConfig, StateBuilder,
};
pub use discovery::{
    DiscoveryClient, MatchedProvider, ProviderRecommendation, StorageRequirements,
};
pub use provider::{ProviderClient, ProviderSettings};
pub use signer::Signer;
pub use storage_user::{
    CheckpointSignatureResponse, CommitResponse, CommitmentResponse, ExistsResponse,
    HealthResponse, StorageUserClient,
};
pub use verification::ClientVerifier;

// Commitment / ChunkLocation appear in the public challenge & checkpoint method
// signatures, so re-export them rather than make callers depend on
// storage_primitives directly.
pub use storage_primitives::{ChunkLocation, Commitment, Visibility};

// Encryption re-exports
pub use encryption::{Cipher, EncryptionKey, XChaCha20Poly1305Cipher, ENCRYPTION_OVERHEAD};
