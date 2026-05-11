//! # Storage Client SDK
//!
//! Comprehensive off-chain SDK for interacting with the scalable Web3 storage system.
//!
//! ## Architecture
//!
//! The SDK provides specialized clients for different user roles:
//!
//! ### For Storage Users
//! [`StorageUserClient`](storage_user::StorageUserClient) - Upload, download, and verify data
//! ```no_run
//! use storage_client::{StorageUserClient, ClientConfig, ChunkingStrategy};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = StorageUserClient::with_defaults()?;
//!
//! // Upload data
//! let data = b"Hello, decentralized world!";
//! let data_root = client.upload(1, data, ChunkingStrategy::default()).await?;
//!
//! // Commit to chain
//! let commitment = client.commit(1, vec![data_root]).await?;
//!
//! // Download and verify
//! let retrieved = client.download(&data_root, 0, data.len() as u64).await?;
//! assert_eq!(retrieved, data);
//! # Ok(())
//! # }
//! ```
//!
//! ### For Storage Providers
//! [`ProviderClient`](provider::ProviderClient) - Manage provider operations
//! ```no_run
//! use storage_client::{ProviderClient, ClientConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = ProviderClient::with_defaults("5GrwvaEF...".to_string())?;
//!
//! // Register as provider
//! client.register(
//!     "/ip4/1.2.3.4/tcp/3333".to_string(),
//!     vec![0u8; 32], // public key
//!     1_000_000_000_000, // stake
//! ).await?;
//!
//! // Accept agreements
//! client.accept_agreement(1).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### For Bucket Administrators
//! [`AdminClient`](admin::AdminClient) - Manage buckets and agreements
//! ```no_run
//! use storage_client::{AdminClient, ClientConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = AdminClient::with_defaults("5GrwvaEF...".to_string())?;
//!
//! // Create bucket
//! let bucket_id = client.create_bucket(2).await?;
//!
//! // Request storage
//! client.request_agreement(
//!     bucket_id,
//!     "5FHneW46...".to_string(),
//!     10 * 1024 * 1024 * 1024, // 10 GB
//!     100_000, // blocks
//!     1_000_000_000_000, // payment
//!     None,
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### For Data Integrity Monitors
//! [`ChallengerClient`](challenger::ChallengerClient) - Challenge providers
//! ```no_run
//! use storage_client::{ChallengerClient, ClientConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = ChallengerClient::with_defaults("5GrwvaEF...".to_string())?;
//!
//! // Challenge a provider
//! let challenge_id = client.challenge_checkpoint(
//!     1, // bucket_id
//!     "5FHneW46...".to_string(), // provider
//!     5, // leaf_index
//!     123, // chunk_index
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
pub mod event_subscription;
pub mod provider;
pub mod scale_decode;
pub mod storage_user;
pub mod substrate;
pub mod verification;

// Re-export commonly used types
pub use admin::AdminClient;
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
pub use event_subscription::{
    subscribe_bucket_events, subscribe_challenges, subscribe_checkpoints, subscribe_with_callback,
    EventCallback, EventFilter, EventParser, EventStream, EventSubscriber, StorageEvent,
    StorageProviderEventParser, SubscriptionHandle,
};
pub use provider::{ProviderClient, ProviderSettings};
pub use storage_user::{
    CheckpointSignatureResponse, CommitResponse, CommitmentResponse, ExistsResponse,
    HealthResponse, StorageUserClient,
};
pub use verification::ClientVerifier;

// Encryption re-exports
pub use encryption::{Cipher, EncryptionKey, XChaCha20Poly1305Cipher, ENCRYPTION_OVERHEAD};
