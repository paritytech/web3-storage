// SPDX-License-Identifier: Apache-2.0

//! File System Client SDK
//!
//! High-level API for interacting with the Layer 1 file system built on top of
//! Scalable Web3 Storage (Layer 0).
//!
//! # Features
//!
//! A drive is a Layer 0 bucket of `pallet-storage-provider`, identified by
//! its bucket id. The chain stores no drive name and no drive record. Sharing a
//! drive means adding the account as a bucket member.
//!
//! - Drive creation (`create_bucket_with_primary`) and membership
//! - File operations (upload, download, delete)
//! - Directory operations (create, list, delete)
//! - Checkpoints of the drive's bucket
//!
//! Files and directories are blobs in the drive's bucket, written and read
//! through Layer 0 routes only. The root directory is the bucket's last MMR
//! leaf, so any client instance can open a drive. See [`tree`] for the
//! format, the write order and the limits (single writer, unauthenticated
//! reads).
//!
//! # Example
//!
//! ```ignore
//! use file_system_client::{FileSystemClient, Signer};
//!
//! let fs_client = FileSystemClient::new(
//!     "ws://127.0.0.1:2222",
//!     "http://127.0.0.1:3333",
//!     Signer::from_seed("//Alice")?,
//! ).await?;
//!
//! // `signed` comes from `ProviderClient::negotiate_terms`.
//! let bucket_id = fs_client
//!     .create_drive(provider, signed.terms, signed.signature, Visibility::Private)
//!     .await?;
//!
//! fs_client
//!     .upload_file(bucket_id, "/documents/report.pdf", &file_bytes, Some("application/pdf"))
//!     .await?;
//! let entries = fs_client.list_directory(bucket_id, "/documents").await?;
//! let bytes = fs_client.download_file(bucket_id, "/documents/report.pdf").await?;
//! ```

mod substrate;
pub mod tree;

use file_system_primitives::{Cid, DirectoryEntry, DEFAULT_MIME_TYPE};
use sp_runtime::AccountId32;
use std::sync::Arc;
use storage_client::{
    BatchedCheckpointConfig, BatchedInterval, CheckpointCallback, CheckpointLoopHandle,
    CheckpointManager, ClientConfig, StorageUserClient,
};
use storage_subxt::api::storage_provider::events::BucketCreated;
use thiserror::Error;
use tokio::sync::Mutex;

pub use storage_client::{CheckpointConfig, CheckpointResult, Signer};
pub use storage_primitives::{BucketId, Role};
pub use substrate::SubstrateClient;
pub use tree::{BlobLength, BlobStore, EmptyParents, FileContent, FileStat, Tree};

/// File system client errors
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsClientError {
    #[error("Storage client error: {0}")]
    StorageClient(String),

    #[error("Path not found: {0}")]
    PathNotFound(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Entry already exists: {0}")]
    EntryExists(String),

    #[error("Not a directory: {0}")]
    NotADirectory(String),

    #[error("Not a file: {0}")]
    NotAFile(String),

    /// The directory has entries; delete them first.
    #[error("Directory not empty: {0}")]
    DirectoryNotEmpty(String),

    /// The bucket's last MMR leaf is not a root directory of this drive.
    /// Something other than the file system or S3 clients committed to the
    /// bucket.
    #[error("Bucket {bucket_id} is not a file system bucket: {reason}")]
    NotAFileSystemBucket {
        /// The bucket that was read.
        bucket_id: BucketId,
        /// What is wrong with the last leaf.
        reason: String,
    },

    /// The provider's MMR proof is not a proof of the last leaf of its
    /// commitment. A commit by another writer between the two requests also
    /// causes it; retry the operation.
    #[error("Invalid MMR proof for the last leaf of bucket {0}")]
    InvalidMmrProof(BucketId),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Blockchain error: {0}")]
    Blockchain(String),

    #[error("Event not found in transaction")]
    EventNotFound,

    #[error("Bounded collection overflow")]
    BoundedOverflow,

    #[error("No signer configured")]
    NoSigner,

    #[error("Configuration error: {0}")]
    Config(String),

    /// A blob's data root is not the CID that references it.
    #[error("CID mismatch: expected {expected:?}, got {got:?}")]
    CidMismatch {
        /// The CID that references the blob.
        expected: Cid,
        /// The data root of the bytes read or uploaded.
        got: Cid,
    },
}

pub type Result<T> = std::result::Result<T, FsClientError>;

/// High-level file system client
pub struct FileSystemClient {
    /// Layer 0 storage client for blob operations
    storage_client: StorageUserClient,
    /// Substrate blockchain client
    substrate_client: SubstrateClient,
    /// Background checkpoint loop handle (if automatic checkpointing is enabled)
    checkpoint_handle: Option<Arc<Mutex<CheckpointLoopHandle>>>,
}

impl FileSystemClient {
    /// Create a new file system client
    ///
    /// # Arguments
    ///
    /// * `chain_endpoint` - Parachain WebSocket RPC endpoint (e.g., "ws://127.0.0.1:2222")
    /// * `provider_endpoint` - Storage provider HTTP endpoint
    ///
    /// The `signer` authenticates provider requests and signs on-chain
    /// extrinsics; build it via [`Signer::from_seed`] (e.g. `"//Alice"`) or
    /// [`Signer::from_keypair`].
    pub async fn new(
        chain_endpoint: &str,
        provider_endpoint: &str,
        signer: Signer,
    ) -> Result<Self> {
        let storage_client = StorageUserClient::new(
            ClientConfig {
                provider_urls: vec![provider_endpoint.to_string()],
                ..Default::default()
            },
            signer.clone(),
        )
        .map_err(|e| FsClientError::Config(e.to_string()))?;
        let substrate_client = SubstrateClient::connect(chain_endpoint, signer).await?;

        Ok(Self {
            storage_client,
            substrate_client,
            checkpoint_handle: None,
        })
    }

    /// Create a drive: a Layer 0 bucket with one primary agreement.
    ///
    /// Submits `StorageProvider::create_bucket_with_primary`, which creates the
    /// bucket and opens the primary agreement in one call. The signer becomes
    /// the bucket admin.
    ///
    /// * `provider` - Provider account that signed `terms`
    /// * `terms` - Provider-signed agreement terms (from `ProviderClient::negotiate_terms`)
    /// * `sig` - Provider signature over the SCALE-encoded terms
    /// * `visibility` - Bucket read visibility
    ///
    /// Returns the bucket id. Use it as the drive id in every other call.
    /// The drive starts empty; nothing is stored until the first write.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use storage_client::{NegotiateRequest, ProviderClient};
    ///
    /// let nonce = admin_client.agreement_nonce(&owner_account).await?;
    /// let signed = ProviderClient::negotiate_terms(
    ///     "http://127.0.0.1:3333",
    ///     &NegotiateRequest {
    ///         owner: owner_account,
    ///         max_bytes: 10_000_000_000,
    ///         duration: 500,
    ///         price_per_byte: 1,
    ///         nonce,
    ///         replica_params: None,
    ///         bucket: None,
    ///     },
    /// ).await?;
    ///
    /// let bucket_id = fs_client.create_drive(
    ///     provider_account,
    ///     signed.terms,
    ///     signed.signature,
    ///     storage_client::Visibility::Private,
    /// ).await?;
    /// ```
    pub async fn create_drive(
        &self,
        provider: AccountId32,
        terms: storage_client::AgreementTermsOf,
        sig: sp_runtime::MultiSignature,
        visibility: storage_client::Visibility,
    ) -> Result<BucketId> {
        self.create_bucket_on_chain(provider, &terms, &sig, visibility)
            .await
    }

    /// Add `member` to the drive's bucket with `role`, or change its role.
    ///
    /// Submits `StorageProvider::set_member`. Only a bucket admin may call it.
    pub async fn add_member(
        &self,
        bucket_id: BucketId,
        member: AccountId32,
        role: Role,
    ) -> Result<()> {
        let call = storage_client::substrate::extrinsics::set_member(bucket_id, member, role);
        self.substrate_client.submit(&call).await?;
        Ok(())
    }

    /// Remove `member` from the drive's bucket.
    ///
    /// Submits `StorageProvider::remove_member`. Only a bucket admin may call it.
    pub async fn remove_member(&self, bucket_id: BucketId, member: AccountId32) -> Result<()> {
        let call = storage_client::substrate::extrinsics::remove_bucket_member(bucket_id, member);
        self.substrate_client.submit(&call).await?;
        Ok(())
    }

    /// The file tree of `bucket_id`, read and written through this client's
    /// provider.
    pub fn tree(&self, bucket_id: BucketId) -> Tree<&StorageUserClient> {
        Tree::new(&self.storage_client, bucket_id)
    }

    /// Write `data` as the file at `path` (e.g. `/documents/report.pdf`).
    ///
    /// Creates missing parent directories and replaces an existing file.
    /// `content_type` `None` or `""` stores `application/octet-stream`. Fails with
    /// [`FsClientError::NotAFile`] if `path` is a directory.
    pub async fn upload_file(
        &self,
        bucket_id: BucketId,
        path: &str,
        data: &[u8],
        content_type: Option<&str>,
    ) -> Result<FileStat> {
        let stat = self
            .tree(bucket_id)
            .put_file(
                path,
                data,
                content_type.unwrap_or(DEFAULT_MIME_TYPE),
                Vec::new(),
            )
            .await?;
        self.mark_drive_dirty(bucket_id).await?;
        Ok(stat)
    }

    /// Read the file at `path`. Checks the bytes against the file's content
    /// root.
    pub async fn download_file(&self, bucket_id: BucketId, path: &str) -> Result<Vec<u8>> {
        Ok(self.get_file(bucket_id, path).await?.data)
    }

    /// Read the file at `path` with its content type, size and mtime.
    pub async fn get_file(&self, bucket_id: BucketId, path: &str) -> Result<FileContent> {
        self.tree(bucket_id).get_file(path).await
    }

    /// List the entries of the directory at `path`, sorted by name.
    pub async fn list_directory(
        &self,
        bucket_id: BucketId,
        path: &str,
    ) -> Result<Vec<DirectoryEntry>> {
        self.tree(bucket_id).list(path).await
    }

    /// Create an empty directory at `path`, and any missing parents. Fails
    /// with [`FsClientError::EntryExists`] if `path` exists.
    pub async fn create_directory(&self, bucket_id: BucketId, path: &str) -> Result<()> {
        self.tree(bucket_id).mkdir(path).await?;
        self.mark_drive_dirty(bucket_id).await
    }

    /// Delete the file or empty directory at `path`. Fails with
    /// [`FsClientError::DirectoryNotEmpty`] for a directory with entries and
    /// [`FsClientError::PathNotFound`] if `path` does not exist. The root
    /// cannot be deleted. The deleted blobs remain in the bucket's history.
    pub async fn delete(&self, bucket_id: BucketId, path: &str) -> Result<()> {
        self.tree(bucket_id)
            .delete(path, EmptyParents::Keep)
            .await?;
        self.mark_drive_dirty(bucket_id).await
    }

    /// CID of the drive's root directory: the data root of the bucket's last
    /// MMR leaf. `None` if the drive has no commits yet.
    pub async fn get_root_cid(&self, bucket_id: BucketId) -> Result<Option<Cid>> {
        Ok(self.tree(bucket_id).load_root().await?.cid)
    }

    // ============ Checkpoint Methods ============

    /// Submit a checkpoint for a drive.
    ///
    /// This coordinates with all storage providers to collect their signed commitments,
    /// verifies consensus (majority agreement), and submits the checkpoint on-chain.
    ///
    /// The checkpoint proves that providers have committed to storing the data,
    /// creating non-repudiable evidence that can be used for challenges if needed.
    ///
    /// # Arguments
    ///
    /// * `bucket_id` - The drive to checkpoint
    /// * `provider_endpoints` - HTTP endpoints of providers to collect commitments from
    ///
    /// # Returns
    ///
    /// `CheckpointResult` indicating success or the reason for failure
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Single provider setup (development/testing)
    /// let result = fs_client.submit_checkpoint(
    ///     bucket_id,
    ///     vec!["http://127.0.0.1:3333".to_string()],
    /// ).await;
    ///
    /// match result {
    ///     CheckpointResult::Submitted { block_hash, signers } => {
    ///         println!("Checkpoint submitted! {} providers signed", signers.len());
    ///     }
    ///     CheckpointResult::InsufficientConsensus { agreeing, required, .. } => {
    ///         println!("Not enough providers agreed: {}/{}", agreeing, required);
    ///     }
    ///     CheckpointResult::TransactionFailed { error } => {
    ///         println!("Transaction failed: {}", error);
    ///     }
    ///     _ => {}
    /// }
    /// ```
    pub async fn submit_checkpoint(
        &self,
        bucket_id: BucketId,
        provider_endpoints: Vec<String>,
    ) -> Result<CheckpointResult> {
        // Get chain endpoint from our substrate client
        let chain_endpoint = self.substrate_client.endpoint();

        // Create checkpoint manager
        let manager = CheckpointManager::new(chain_endpoint, CheckpointConfig::default())
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        // Configure with provider endpoints
        let manager = manager.with_providers(provider_endpoints);

        // Use the same signer as the file system client
        let manager = manager.with_signer(self.substrate_client.signer().clone());

        // Submit checkpoint
        Ok(manager.submit_checkpoint(bucket_id).await)
    }

    /// Submit a checkpoint with a custom configuration.
    ///
    /// Use this when you need to customize timeouts, retry behavior, or consensus thresholds.
    pub async fn submit_checkpoint_with_config(
        &self,
        bucket_id: BucketId,
        provider_endpoints: Vec<String>,
        config: CheckpointConfig,
    ) -> Result<CheckpointResult> {
        let chain_endpoint = self.substrate_client.endpoint();

        let manager = CheckpointManager::new(chain_endpoint, config)
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        let manager = manager.with_providers(provider_endpoints);

        let manager = manager.with_signer(self.substrate_client.signer().clone());

        Ok(manager.submit_checkpoint(bucket_id).await)
    }

    // ============ Automatic Checkpoint Methods ============

    /// Enable automatic batched checkpoints for a drive.
    ///
    /// This starts a background loop that periodically submits checkpoints.
    /// Changes are automatically tracked, and checkpoints are submitted when
    /// the interval elapses.
    ///
    /// # Arguments
    ///
    /// * `bucket_id` - The drive to enable automatic checkpoints for
    /// * `provider_endpoints` - HTTP endpoints of storage providers
    /// * `interval_blocks` - Number of blocks between checkpoints (default: 100)
    /// * `callback` - Optional callback invoked after each checkpoint attempt
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Enable automatic checkpoints every 100 blocks
    /// fs_client.enable_auto_checkpoints(
    ///     bucket_id,
    ///     vec!["http://127.0.0.1:3333".to_string()],
    ///     Some(100),
    ///     Some(Arc::new(|bucket_id, result| {
    ///         match result {
    ///             CheckpointResult::Submitted { .. } => println!("Checkpoint submitted!"),
    ///             _ => println!("Checkpoint failed: {:?}", result),
    ///         }
    ///     })),
    /// ).await?;
    ///
    /// // Now file operations will automatically mark the drive as dirty
    /// fs_client.upload_file(bucket_id, "/file.txt", data, None).await?;
    ///
    /// // Disable when done
    /// fs_client.disable_auto_checkpoints().await?;
    /// ```
    pub async fn enable_auto_checkpoints(
        &mut self,
        bucket_id: BucketId,
        provider_endpoints: Vec<String>,
        interval_blocks: Option<u32>,
        callback: Option<CheckpointCallback>,
    ) -> Result<()> {
        // Stop existing checkpoint loop if any
        self.disable_auto_checkpoints().await?;

        // Get chain endpoint
        let chain_endpoint = self.substrate_client.endpoint();

        // Create checkpoint manager
        let manager = CheckpointManager::new(chain_endpoint, CheckpointConfig::default())
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        // Configure with provider endpoints
        let manager = manager.with_providers(provider_endpoints);

        // Use the same signer as the file system client
        let manager = manager.with_signer(self.substrate_client.signer().clone());

        // Configure batched checkpoint loop
        let batched_config = BatchedCheckpointConfig {
            interval: BatchedInterval::Blocks(interval_blocks.unwrap_or(100)),
            submit_on_empty: false,
            max_consecutive_failures: 5,
            failure_retry_delay: std::time::Duration::from_secs(30),
        };

        // Start the background loop
        let handle = Arc::new(manager)
            .start_checkpoint_loop(bucket_id, batched_config, callback)
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        self.checkpoint_handle = Some(Arc::new(Mutex::new(handle)));

        Ok(())
    }

    /// Disable automatic checkpoints.
    ///
    /// Stops the background checkpoint loop. Any pending changes will not be
    /// automatically checkpointed - you should call `submit_checkpoint()` manually
    /// if needed before disabling.
    pub async fn disable_auto_checkpoints(&mut self) -> Result<()> {
        if let Some(handle) = self.checkpoint_handle.take() {
            let mut guard = handle.lock().await;
            guard
                .stop()
                .await
                .map_err(|e| FsClientError::StorageClient(e.to_string()))?;
        }
        Ok(())
    }

    /// Request immediate checkpoint submission.
    ///
    /// This is useful when you want to force a checkpoint outside the normal
    /// batched interval, for example before a critical operation.
    pub async fn request_immediate_checkpoint(&self) -> Result<()> {
        if let Some(handle) = &self.checkpoint_handle {
            let guard = handle.lock().await;
            guard
                .submit_now()
                .await
                .map_err(|e| FsClientError::StorageClient(e.to_string()))?;
        }
        Ok(())
    }

    /// Check if automatic checkpoints are enabled.
    pub fn is_auto_checkpoints_enabled(&self) -> bool {
        self.checkpoint_handle.is_some()
    }

    /// Mark a drive as having pending changes.
    ///
    /// This is called automatically by file operations when auto-checkpoints
    /// are enabled, but can also be called manually if needed.
    async fn mark_drive_dirty(&self, bucket_id: BucketId) -> Result<()> {
        if let Some(handle) = &self.checkpoint_handle {
            let guard = handle.lock().await;
            guard
                .mark_dirty(bucket_id)
                .await
                .map_err(|e| FsClientError::StorageClient(e.to_string()))?;
        }
        Ok(())
    }

    // ============ Chain Interaction ============

    /// Submit `create_bucket_with_primary` and return the new bucket id from
    /// the `BucketCreated` event.
    async fn create_bucket_on_chain(
        &self,
        provider: AccountId32,
        terms: &storage_client::AgreementTermsOf,
        sig: &sp_runtime::MultiSignature,
        visibility: storage_client::Visibility,
    ) -> Result<BucketId> {
        let call = storage_client::substrate::extrinsics::create_bucket_with_primary(
            provider, terms, sig, visibility,
        );
        let events = self.substrate_client.submit(&call).await.inspect_err(|e| {
            tracing::error!("create_bucket_with_primary failed: {e}");
        })?;

        let created = events
            .find_first::<BucketCreated>()
            .ok_or(FsClientError::EventNotFound)?
            .map_err(|e| {
                FsClientError::Blockchain(format!("Failed to decode BucketCreated event: {e}"))
            })?;

        tracing::info!("Drive created (bucket {})", created.bucket_id);
        Ok(created.bucket_id)
    }
}
