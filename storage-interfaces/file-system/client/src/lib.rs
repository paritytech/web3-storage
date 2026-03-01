//! File System Client SDK
//!
//! High-level API for interacting with the Layer 1 file system built on top of
//! Scalable Web3 Storage (Layer 0).
//!
//! # Features
//!
//! - Drive management (create, list, delete)
//! - File operations (upload, download, delete)
//! - Directory operations (create, list, traverse)
//! - DAG navigation and CID resolution
//! - Automatic root CID updates on changes
//!
//! # Example
//!
//! ```ignore
//! use file_system_client::{FileSystemClient, DriveId};
//!
//! // Create client
//! let fs_client = FileSystemClient::new(
//!     "http://localhost:9944",
//!     "http://provider.example.com",
//! ).await?;
//!
//! // Create a new drive
//! let drive_id = fs_client.create_drive(bucket_id, "My Drive").await?;
//!
//! // Upload a file
//! fs_client.upload_file(drive_id, "/documents/report.pdf", file_bytes).await?;
//!
//! // List directory
//! let entries = fs_client.list_directory(drive_id, "/documents").await?;
//!
//! // Download a file
//! let bytes = fs_client.download_file(drive_id, "/documents/report.pdf").await?;
//! ```

mod substrate;

use file_system_primitives::{
    compute_cid, Cid, DirectoryEntry, DirectoryNode, EntryType, FileManifest,
};
use sp_core::H256;
use sp_runtime::BoundedVec;
use std::collections::HashMap;
use std::sync::Arc;
use storage_client::{
    BatchedCheckpointConfig, BatchedInterval, CheckpointCallback, CheckpointLoopHandle,
    CheckpointManager, StorageClient,
};
use thiserror::Error;
use tokio::sync::Mutex;

pub use file_system_primitives::DriveId;
pub use storage_client::{CheckpointConfig, CheckpointResult};
pub use substrate::SubstrateClient;

/// File system client errors
#[derive(Debug, Error)]
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

    #[error("Drive not found: {0}")]
    DriveNotFound(DriveId),

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
}

pub type Result<T> = std::result::Result<T, FsClientError>;

/// High-level file system client
pub struct FileSystemClient {
    /// Layer 0 storage client for blob operations
    storage_client: StorageClient,
    /// Substrate blockchain client
    substrate_client: SubstrateClient,
    /// In-memory cache of drive root CIDs (drive_id -> root_cid)
    root_cache: HashMap<DriveId, Cid>,
    /// Background checkpoint loop handle (if automatic checkpointing is enabled)
    checkpoint_handle: Option<Arc<Mutex<CheckpointLoopHandle>>>,
    /// Mapping of drive_id to bucket_id for automatic checkpointing
    drive_bucket_map: HashMap<DriveId, u64>,
}

impl FileSystemClient {
    /// Create a new file system client
    ///
    /// # Arguments
    ///
    /// * `chain_endpoint` - Parachain WebSocket RPC endpoint (e.g., "ws://localhost:9944")
    /// * `provider_endpoint` - Storage provider HTTP endpoint
    pub async fn new(chain_endpoint: &str, provider_endpoint: &str) -> Result<Self> {
        let storage_client = StorageClient::new(provider_endpoint);
        let substrate_client = SubstrateClient::connect(chain_endpoint).await?;

        Ok(Self {
            storage_client,
            substrate_client,
            root_cache: HashMap::new(),
            checkpoint_handle: None,
            drive_bucket_map: HashMap::new(),
        })
    }

    /// Create a client with a development signer (for testing).
    pub async fn with_dev_signer(mut self, name: &str) -> Result<Self> {
        self.substrate_client = self.substrate_client.with_dev_signer(name)?;
        Ok(self)
    }

    /// Set a custom signer for blockchain transactions.
    pub fn with_signer(mut self, signer: subxt_signer::sr25519::Keypair) -> Self {
        self.substrate_client = self.substrate_client.with_signer(signer);
        self
    }

    /// Create a new drive (USER-FACING API)
    ///
    /// This is the primary way for users to create drives. The system automatically:
    /// - Creates a bucket in Layer 0
    /// - Requests storage agreements with providers
    /// - Sets up the drive infrastructure
    ///
    /// Users don't need to understand buckets, agreements, or providers - they just
    /// specify their storage requirements and get a drive!
    ///
    /// # Arguments
    ///
    /// * `name` - Optional human-readable name for the drive
    /// * `max_capacity` - Maximum storage capacity in bytes (e.g., 10 GB = 10_000_000_000)
    /// * `storage_period` - Storage duration in blocks (e.g., 500 blocks)
    /// * `payment` - Upfront payment tokens for storage agreements
    /// * `min_providers` - Optional minimum number of providers (default: 3 for long-term, 1 for short-term)
    /// * `commit_strategy` - Optional strategy for committing changes (default: Batched every 100 blocks)
    ///
    /// # Returns
    ///
    /// The newly created drive ID
    ///
    /// # Example
    ///
    /// ```ignore
    /// use file_system_primitives::CommitStrategy;
    ///
    /// // Create a 10 GB drive with defaults
    /// let drive_id = fs_client.create_drive(
    ///     Some("My Documents"),
    ///     10_000_000_000,  // 10 GB
    ///     500,              // 500 blocks
    ///     1_000_000_000_000, // 1 token (12 decimals)
    ///     None,             // Use default providers (auto-determined)
    ///     None,             // Use default commit strategy
    /// ).await?;
    ///
    /// // Create a highly replicated drive with immediate commits
    /// let drive_id = fs_client.create_drive(
    ///     Some("Critical Data"),
    ///     5_000_000_000,
    ///     500,
    ///     2_000_000_000_000, // 2 tokens for more providers
    ///     Some(5),           // 1 primary + 4 replicas
    ///     Some(CommitStrategy::Immediate),
    /// ).await?;
    /// ```
    pub async fn create_drive(
        &mut self,
        name: Option<&str>,
        max_capacity: u64,
        storage_period: u64,
        payment: u128,
        min_providers: Option<u8>,
        commit_strategy: Option<file_system_primitives::CommitStrategy>,
    ) -> Result<DriveId> {
        // Call on-chain extrinsic to create drive
        // The system automatically:
        // 1. Creates a bucket in Layer 0
        // 2. Requests storage agreements with providers
        // 3. Creates an empty root directory
        // 4. Returns the drive_id
        //
        // NOTE: In a real implementation, this would use subxt or similar to call:
        // drive_registry.create_drive(name, max_capacity, storage_period, payment, min_providers, commit_strategy)

        // Use provided strategy or default
        let strategy = commit_strategy.unwrap_or_default();

        let drive_id = self
            .create_drive_on_chain(
                name,
                max_capacity,
                storage_period,
                payment,
                min_providers,
                strategy,
            )
            .await?;

        // Get the bucket_id for this drive
        let bucket_id = self.query_drive_bucket_id(drive_id).await?;

        // Create an empty root directory and upload it
        let root_dir = DirectoryNode::new_empty(drive_id);
        let root_dir_bytes = root_dir.to_scale_bytes();

        // Upload the root directory to the provider
        // The returned data_root is the hash of the data, which we use as the root CID
        let root_cid = self.upload_blob(bucket_id, &root_dir_bytes).await?;

        // Verify the CID matches what we would compute locally
        let expected_cid = compute_cid(&root_dir_bytes);
        if root_cid != expected_cid {
            log::warn!("CID mismatch: data_root={root_cid:?}, expected={expected_cid:?}");
        }

        // Update the on-chain root CID
        self.update_drive_root_cid(drive_id, root_cid).await?;

        // Cache the root CID
        log::debug!("create_drive: caching root_cid={root_cid:?} for drive {drive_id}");
        self.root_cache.insert(drive_id, root_cid);

        Ok(drive_id)
    }

    /// Upload a file to the file system
    ///
    /// # Arguments
    ///
    /// * `drive_id` - Target drive
    /// * `path` - File path (e.g., "/documents/report.pdf")
    /// * `data` - File contents
    /// * `bucket_id` - Bucket to store file chunks
    pub async fn upload_file(
        &mut self,
        drive_id: DriveId,
        path: &str,
        data: &[u8],
        bucket_id: u64,
    ) -> Result<()> {
        // Validate and parse path
        let (parent_path, file_name) = Self::split_path(path)?;

        // Split file into chunks (256 KiB chunks)
        const CHUNK_SIZE: usize = 256 * 1024;
        let mut manifest = FileManifest {
            drive_id,
            mime_type: BoundedVec::try_from(Self::guess_mime_type(file_name).into_bytes())
                .map_err(|_| FsClientError::BoundedOverflow)?,
            total_size: data.len() as u64,
            chunks: BoundedVec::default(),
            encryption_params: BoundedVec::default(),
        };

        for (i, chunk_data) in data.chunks(CHUNK_SIZE).enumerate() {
            // Upload chunk and use the returned data_root as the chunk CID
            let chunk_cid = self.upload_blob(bucket_id, chunk_data).await?;

            manifest
                .add_chunk(chunk_cid, i as u32)
                .map_err(|_| FsClientError::BoundedOverflow)?;
        }

        let manifest_bytes = manifest.to_scale_bytes();
        // Upload manifest and use the returned data_root as the file CID
        let file_cid = self.upload_blob(bucket_id, &manifest_bytes).await?;

        // Update parent directory
        self.add_entry_to_directory(
            drive_id,
            parent_path,
            file_name,
            file_cid,
            data.len() as u64,
            EntryType::File,
            bucket_id,
        )
        .await?;

        // Mark drive as dirty for automatic checkpointing
        self.mark_drive_dirty(drive_id).await?;

        Ok(())
    }

    /// Download a file from the file system
    ///
    /// # Returns
    ///
    /// The file contents as bytes
    pub async fn download_file(&mut self, drive_id: DriveId, path: &str) -> Result<Vec<u8>> {
        // Navigate to file
        let file_cid = self.resolve_path(drive_id, path).await?;

        // Fetch FileManifest
        let manifest_bytes = self.fetch_blob(file_cid).await?;
        let manifest = FileManifest::from_scale_bytes(&manifest_bytes)
            .map_err(|e| FsClientError::Serialization(format!("Invalid manifest: {e:?}")))?;

        // Validate it's a file
        if manifest.chunks.is_empty() {
            return Err(FsClientError::NotAFile(path.to_string()));
        }

        // Fetch and reassemble chunks
        let mut file_data = Vec::with_capacity(manifest.total_size as usize);

        for chunk in manifest.chunks.iter() {
            let chunk_data = self.fetch_blob(chunk.cid).await?;
            file_data.extend_from_slice(&chunk_data);
        }

        Ok(file_data)
    }

    /// List entries in a directory
    ///
    /// # Returns
    ///
    /// Vector of directory entries
    pub async fn list_directory(
        &mut self,
        drive_id: DriveId,
        path: &str,
    ) -> Result<Vec<DirectoryEntry>> {
        // Navigate to directory
        let dir_cid = self.resolve_path(drive_id, path).await?;

        // Fetch DirectoryNode
        let dir_bytes = self.fetch_blob(dir_cid).await?;
        let dir_node = DirectoryNode::from_scale_bytes(&dir_bytes)
            .map_err(|e| FsClientError::Serialization(format!("Invalid directory: {e:?}")))?;

        Ok(dir_node.children.into_inner())
    }

    /// Create a directory
    pub async fn create_directory(
        &mut self,
        drive_id: DriveId,
        path: &str,
        bucket_id: u64,
    ) -> Result<()> {
        let (parent_path, dir_name) = Self::split_path(path)?;

        // Create empty directory and upload it
        let new_dir = DirectoryNode::new_empty(drive_id);
        let new_dir_bytes = new_dir.to_scale_bytes();
        let new_dir_cid = self.upload_blob(bucket_id, &new_dir_bytes).await?;

        // Add to parent directory
        self.add_entry_to_directory(
            drive_id,
            parent_path,
            dir_name,
            new_dir_cid,
            0,
            EntryType::Directory,
            bucket_id,
        )
        .await?;

        // Mark drive as dirty for automatic checkpointing
        self.mark_drive_dirty(drive_id).await?;

        Ok(())
    }

    /// Get the root CID of a drive
    pub async fn get_root_cid(&mut self, drive_id: DriveId) -> Result<Cid> {
        // Check cache first
        if let Some(cid) = self.root_cache.get(&drive_id) {
            log::debug!("get_root_cid: cache hit for drive {drive_id}, cid={cid:?}");
            return Ok(*cid);
        }

        // Query on-chain
        let cid = self.query_drive_root_cid(drive_id).await?;
        log::debug!("get_root_cid: cache miss for drive {drive_id}, queried cid={cid:?}");
        self.root_cache.insert(drive_id, cid);

        Ok(cid)
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
    /// * `drive_id` - The drive to checkpoint
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
    ///     drive_id,
    ///     vec!["http://localhost:3000".to_string()],
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
        drive_id: DriveId,
        provider_endpoints: Vec<String>,
    ) -> Result<CheckpointResult> {
        // Get the bucket_id for this drive
        let bucket_id = self.query_drive_bucket_id(drive_id).await?;

        // Get chain endpoint from our substrate client
        let chain_endpoint = self.substrate_client.endpoint();

        // Create checkpoint manager
        let manager = CheckpointManager::new(chain_endpoint, CheckpointConfig::default())
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        // Configure with provider endpoints
        let manager = manager.with_providers(provider_endpoints);

        // Use the same signer as the file system client
        let manager = if let Ok(signer) = self.substrate_client.signer_keypair() {
            manager.with_signer(signer.clone())
        } else {
            manager
        };

        // Submit checkpoint
        Ok(manager.submit_checkpoint(bucket_id).await)
    }

    /// Submit a checkpoint with a custom configuration.
    ///
    /// Use this when you need to customize timeouts, retry behavior, or consensus thresholds.
    pub async fn submit_checkpoint_with_config(
        &self,
        drive_id: DriveId,
        provider_endpoints: Vec<String>,
        config: CheckpointConfig,
    ) -> Result<CheckpointResult> {
        let bucket_id = self.query_drive_bucket_id(drive_id).await?;
        let chain_endpoint = self.substrate_client.endpoint();

        let manager = CheckpointManager::new(chain_endpoint, config)
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        let manager = manager.with_providers(provider_endpoints);

        let manager = if let Ok(signer) = self.substrate_client.signer_keypair() {
            manager.with_signer(signer.clone())
        } else {
            manager
        };

        Ok(manager.submit_checkpoint(bucket_id).await)
    }

    /// Get the bucket ID for a drive.
    ///
    /// This is useful when you need to interact directly with Layer 0 operations
    /// for a specific drive.
    pub async fn get_bucket_id(&self, drive_id: DriveId) -> Result<u64> {
        self.query_drive_bucket_id(drive_id).await
    }

    // ============ Automatic Checkpoint Methods ============

    /// Enable automatic batched checkpoints for a drive.
    ///
    /// This starts a background loop that periodically submits checkpoints
    /// according to the drive's CommitStrategy. Changes are automatically tracked,
    /// and checkpoints are submitted when the interval elapses.
    ///
    /// # Arguments
    ///
    /// * `drive_id` - The drive to enable automatic checkpoints for
    /// * `provider_endpoints` - HTTP endpoints of storage providers
    /// * `interval_blocks` - Number of blocks between checkpoints (default: 100)
    /// * `callback` - Optional callback invoked after each checkpoint attempt
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Enable automatic checkpoints every 100 blocks
    /// fs_client.enable_auto_checkpoints(
    ///     drive_id,
    ///     vec!["http://localhost:3000".to_string()],
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
    /// fs_client.upload_file(drive_id, "/file.txt", data, bucket_id).await?;
    ///
    /// // Disable when done
    /// fs_client.disable_auto_checkpoints().await?;
    /// ```
    pub async fn enable_auto_checkpoints(
        &mut self,
        drive_id: DriveId,
        provider_endpoints: Vec<String>,
        interval_blocks: Option<u32>,
        callback: Option<CheckpointCallback>,
    ) -> Result<()> {
        // Stop existing checkpoint loop if any
        self.disable_auto_checkpoints().await?;

        // Get the bucket_id for this drive
        let bucket_id = self.query_drive_bucket_id(drive_id).await?;
        self.drive_bucket_map.insert(drive_id, bucket_id);

        // Get chain endpoint
        let chain_endpoint = self.substrate_client.endpoint();

        // Create checkpoint manager
        let manager = CheckpointManager::new(chain_endpoint, CheckpointConfig::default())
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        // Configure with provider endpoints
        let manager = manager.with_providers(provider_endpoints);

        // Use the same signer as the file system client
        let manager = if let Ok(signer) = self.substrate_client.signer_keypair() {
            manager.with_signer(signer.clone())
        } else {
            manager
        };

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
    async fn mark_drive_dirty(&self, drive_id: DriveId) -> Result<()> {
        if let Some(handle) = &self.checkpoint_handle {
            if let Some(&bucket_id) = self.drive_bucket_map.get(&drive_id) {
                let guard = handle.lock().await;
                guard
                    .mark_dirty(bucket_id)
                    .await
                    .map_err(|e| FsClientError::StorageClient(e.to_string()))?;
            }
        }
        Ok(())
    }

    // ============ Internal Helper Methods ============

    /// Resolve a path to a CID by traversing the DAG
    async fn resolve_path(&mut self, drive_id: DriveId, path: &str) -> Result<Cid> {
        let mut current_cid = self.get_root_cid(drive_id).await?;

        // Handle root path
        if path == "/" {
            return Ok(current_cid);
        }

        // Split path into components
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // Traverse path
        for component in components {
            let dir_bytes = self.fetch_blob(current_cid).await?;
            let dir_node = DirectoryNode::from_scale_bytes(&dir_bytes)
                .map_err(|e| FsClientError::Serialization(format!("Invalid directory: {e:?}")))?;

            // Find child entry
            let entry = dir_node
                .find_child(component)
                .ok_or_else(|| FsClientError::PathNotFound(path.to_string()))?;

            current_cid = entry.cid;
        }

        Ok(current_cid)
    }

    /// Add an entry to a directory and update the DAG up to root
    #[allow(clippy::too_many_arguments)]
    async fn add_entry_to_directory(
        &mut self,
        drive_id: DriveId,
        parent_path: &str,
        name: &str,
        cid: Cid,
        size: u64,
        entry_type: EntryType,
        bucket_id: u64,
    ) -> Result<()> {
        // Fetch parent directory
        let parent_cid = self.resolve_path(drive_id, parent_path).await?;
        let parent_bytes = self.fetch_blob(parent_cid).await?;
        let mut parent_node = DirectoryNode::from_scale_bytes(&parent_bytes)
            .map_err(|e| FsClientError::Serialization(format!("Invalid directory: {e:?}")))?;

        // Check if entry already exists
        if parent_node.find_child(name).is_some() {
            return Err(FsClientError::EntryExists(name.to_string()));
        }

        // Add new entry
        let entry = DirectoryEntry {
            name: BoundedVec::try_from(name.as_bytes().to_vec())
                .map_err(|_| FsClientError::BoundedOverflow)?,
            entry_type,
            cid,
            size,
            mtime: Self::current_timestamp(),
        };
        parent_node
            .add_child(entry)
            .map_err(|_| FsClientError::BoundedOverflow)?;

        // Upload updated parent
        let new_parent_bytes = parent_node.to_scale_bytes();
        let new_parent_cid = self.upload_blob(bucket_id, &new_parent_bytes).await?;

        // Update ancestors up to root
        let new_root_cid = self
            .update_ancestors(drive_id, parent_path, new_parent_cid, bucket_id)
            .await?;

        // Update on-chain root CID
        self.update_drive_root_cid(drive_id, new_root_cid).await?;

        // Update cache
        self.root_cache.insert(drive_id, new_root_cid);

        Ok(())
    }

    /// Update all ancestor directories up to root after a change
    async fn update_ancestors(
        &mut self,
        drive_id: DriveId,
        path: &str,
        new_child_cid: Cid,
        bucket_id: u64,
    ) -> Result<Cid> {
        if path == "/" {
            // We've reached root, return the new CID
            return Ok(new_child_cid);
        }

        // Split path
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Ok(new_child_cid);
        }

        // Build parent path
        let child_name = components.last().unwrap();
        let parent_path = if components.len() == 1 {
            "/"
        } else {
            &path[..path.rfind('/').unwrap()]
        };

        // Fetch parent
        let parent_cid = self.resolve_path(drive_id, parent_path).await?;
        let parent_bytes = self.fetch_blob(parent_cid).await?;
        let mut parent_node = DirectoryNode::from_scale_bytes(&parent_bytes)
            .map_err(|e| FsClientError::Serialization(format!("Invalid directory: {e:?}")))?;

        // Update child entry
        if let Some(entry) = parent_node.find_child_mut(child_name) {
            entry.cid = new_child_cid;
            entry.mtime = Self::current_timestamp();
        }

        // Upload updated parent
        let new_parent_bytes = parent_node.to_scale_bytes();
        let new_parent_cid = self.upload_blob(bucket_id, &new_parent_bytes).await?;

        // Recurse to grandparent (box the future to avoid infinite size)
        Box::pin(self.update_ancestors(drive_id, parent_path, new_parent_cid, bucket_id)).await
    }

    /// Upload a blob to Layer 0 storage and return the data root
    async fn upload_blob(&self, bucket_id: u64, data: &[u8]) -> Result<Cid> {
        use storage_client::ChunkingStrategy;

        // Upload data using default chunking strategy
        let data_root = self
            .storage_client
            .upload(bucket_id, data, ChunkingStrategy::default())
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        // The data_root returned by the storage client is the hash of the data
        // which should match our CID for single-chunk uploads
        log::debug!("Uploaded blob, data_root: {data_root:?}");

        Ok(data_root)
    }

    /// Fetch a blob from Layer 0 storage by CID
    async fn fetch_blob(&self, cid: Cid) -> Result<Vec<u8>> {
        // Use the read API with CID as data root
        // Note: This assumes provider maps CID to stored data
        //
        // We use a large but safe maximum length (1 TiB) instead of u64::MAX
        // because the provider's chunk calculation would overflow with u64::MAX:
        //   end_chunk = (offset + length + chunk_size - 1) / chunk_size
        // With u64::MAX, this wraps around and results in end_chunk = 0.
        const MAX_READ_LENGTH: u64 = 1024 * 1024 * 1024 * 1024; // 1 TiB

        let data = self
            .storage_client
            .read(&cid, 0, MAX_READ_LENGTH)
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        log::debug!(
            "fetch_blob: cid={:?}, data_len={}, data_hex={}",
            cid,
            data.len(),
            hex::encode(&data)
        );

        Ok(data)
    }

    /// Split a path into (parent_path, name)
    fn split_path(path: &str) -> Result<(&str, &str)> {
        if !path.starts_with('/') {
            return Err(FsClientError::InvalidPath(
                "Path must start with '/'".to_string(),
            ));
        }

        if path == "/" {
            return Err(FsClientError::InvalidPath(
                "Cannot split root path".to_string(),
            ));
        }

        let last_slash = path.rfind('/').unwrap();
        let parent = if last_slash == 0 {
            "/"
        } else {
            &path[..last_slash]
        };
        let name = &path[last_slash + 1..];

        if name.is_empty() {
            return Err(FsClientError::InvalidPath("Empty name".to_string()));
        }

        Ok((parent, name))
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn guess_mime_type(filename: &str) -> String {
        if filename.ends_with(".pdf") {
            "application/pdf".to_string()
        } else if filename.ends_with(".txt") {
            "text/plain".to_string()
        } else if filename.ends_with(".json") {
            "application/json".to_string()
        } else if filename.ends_with(".png") {
            "image/png".to_string()
        } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
            "image/jpeg".to_string()
        } else {
            "application/octet-stream".to_string()
        }
    }

    // ============ Chain Interaction (Placeholder) ============
    // NOTE: In a real implementation, these would use subxt or similar

    async fn create_drive_on_chain(
        &self,
        name: Option<&str>,
        max_capacity: u64,
        storage_period: u64,
        payment: u128,
        min_providers: Option<u8>,
        commit_strategy: file_system_primitives::CommitStrategy,
    ) -> Result<DriveId> {
        use subxt::dynamic::At;

        let name_bytes = name.map(|n| n.as_bytes().to_vec());

        // Build the extrinsic
        let call = substrate::extrinsics::create_drive(
            name_bytes,
            max_capacity,
            storage_period,
            payment,
            min_providers,
            commit_strategy,
        );

        // Sign and submit
        let signer = self.substrate_client.signer()?;
        let mut progress = self
            .substrate_client
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&call, signer)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Failed to submit tx: {e}")))?;

        // Wait for finalization and extract drive_id from event
        while let Some(event) = progress.next().await {
            let event =
                event.map_err(|e| FsClientError::Blockchain(format!("Transaction error: {e}")))?;

            if let Some(finalized) = event.as_finalized() {
                // Fetch events from the finalized block
                let events = finalized.fetch_events().await.map_err(|e| {
                    FsClientError::Blockchain(format!("Failed to fetch events: {e}"))
                })?;

                // Find DriveCreated or DriveCreatedOnBucket event
                for ev in events.iter() {
                    let ev = ev.map_err(|e| {
                        FsClientError::Blockchain(format!("Event decode error: {e}"))
                    })?;

                    // Check if this is a DriveRegistry event
                    if ev.pallet_name() == "DriveRegistry" {
                        // Try to decode as dynamic value
                        if let Ok(value) = ev.field_values() {
                            // Extract drive_id from first field (all drive events have drive_id as first field)
                            if let Some(drive_id_value) = value.at(0) {
                                if let Some(drive_id) = drive_id_value.as_u128() {
                                    log::info!("Drive created with ID: {drive_id}");
                                    return Ok(drive_id as DriveId);
                                }
                            }
                        }
                    }
                }

                return Err(FsClientError::EventNotFound);
            }
        }

        Err(FsClientError::Blockchain(
            "Transaction did not finalize".to_string(),
        ))
    }

    async fn update_drive_root_cid(&self, drive_id: DriveId, new_root_cid: Cid) -> Result<()> {
        // Build the extrinsic
        let call = substrate::extrinsics::update_root_cid(drive_id, new_root_cid);

        // Sign and submit
        let signer = self.substrate_client.signer()?;
        let mut progress = self
            .substrate_client
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&call, signer)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Failed to submit tx: {e}")))?;

        // Wait for finalization
        while let Some(event) = progress.next().await {
            let event =
                event.map_err(|e| FsClientError::Blockchain(format!("Transaction error: {e}")))?;

            if event.as_finalized().is_some() {
                log::info!("Root CID updated for drive {drive_id}");
                return Ok(());
            }
        }

        Err(FsClientError::Blockchain(
            "Transaction did not finalize".to_string(),
        ))
    }

    async fn query_drive_root_cid(&self, drive_id: DriveId) -> Result<Cid> {
        let storage_client = self
            .substrate_client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Storage query failed: {e}")))?;

        // Build the storage key for Drives storage map
        // Format: pallet_hash + storage_hash + key_hash(drive_id)
        use sp_core::twox_128;

        let pallet_hash = twox_128(b"DriveRegistry");
        let storage_hash = twox_128(b"Drives");
        let key = drive_id.to_le_bytes();
        let key_hash = sp_core::blake2_128(&key);

        let mut storage_key = Vec::new();
        storage_key.extend_from_slice(&pallet_hash);
        storage_key.extend_from_slice(&storage_hash);
        storage_key.extend_from_slice(&key_hash);
        storage_key.extend_from_slice(&key);

        let bytes_opt = storage_client
            .fetch_raw(storage_key)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Storage fetch failed: {e}")))?;

        if let Some(bytes) = bytes_opt {
            // DriveInfo structure:
            // - owner: AccountId32 (32 bytes)
            // - bucket_id: u64 (8 bytes
            // - root_cid: H256 (32 bytes)
            // - ... more fields

            // We need to skip SCALE encoding overhead and extract root_cid
            // This is a simplified approach - in production use proper type decoding

            if bytes.len() >= 32 + 8 + 32 {
                // Skip owner (32 bytes) + bucket_id (8 bytes)
                let root_cid_offset = 32 + 8;
                let mut root_cid_bytes = [0u8; 32];
                root_cid_bytes.copy_from_slice(&bytes[root_cid_offset..root_cid_offset + 32]);
                return Ok(H256::from(root_cid_bytes));
            }

            return Err(FsClientError::Blockchain(
                "Invalid drive info encoding".to_string(),
            ));
        }

        Err(FsClientError::DriveNotFound(drive_id))
    }

    /// Query bucket_id for a drive from on-chain storage
    async fn query_drive_bucket_id(&self, drive_id: DriveId) -> Result<u64> {
        let storage_client = self
            .substrate_client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Storage query failed: {e}")))?;

        // Build the storage key for Drives storage map
        use sp_core::twox_128;

        let pallet_hash = twox_128(b"DriveRegistry");
        let storage_hash = twox_128(b"Drives");
        let key = drive_id.to_le_bytes();
        let key_hash = sp_core::blake2_128(&key);

        let mut storage_key = Vec::new();
        storage_key.extend_from_slice(&pallet_hash);
        storage_key.extend_from_slice(&storage_hash);
        storage_key.extend_from_slice(&key_hash);
        storage_key.extend_from_slice(&key);

        let bytes_opt = storage_client
            .fetch_raw(storage_key)
            .await
            .map_err(|e| FsClientError::Blockchain(format!("Storage fetch failed: {e}")))?;

        if let Some(bytes) = bytes_opt {
            // DriveInfo structure:
            // - owner: AccountId32 (32 bytes)
            // - bucket_id: u64 (8 bytes)
            // - root_cid: H256 (32 bytes)
            // - ... more fields

            if bytes.len() >= 32 + 8 {
                // Skip owner (32 bytes), then read bucket_id (8 bytes)
                let bucket_id_offset = 32;
                let mut bucket_id_bytes = [0u8; 8];
                bucket_id_bytes.copy_from_slice(&bytes[bucket_id_offset..bucket_id_offset + 8]);
                return Ok(u64::from_le_bytes(bucket_id_bytes));
            }

            return Err(FsClientError::Blockchain(
                "Invalid drive info encoding".to_string(),
            ));
        }

        Err(FsClientError::DriveNotFound(drive_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_path() {
        assert_eq!(
            FileSystemClient::split_path("/file.txt").unwrap(),
            ("/", "file.txt")
        );
        assert_eq!(
            FileSystemClient::split_path("/dir/file.txt").unwrap(),
            ("/dir", "file.txt")
        );
        assert_eq!(
            FileSystemClient::split_path("/a/b/c/file.txt").unwrap(),
            ("/a/b/c", "file.txt")
        );
        assert!(FileSystemClient::split_path("/").is_err());
        assert!(FileSystemClient::split_path("no-slash").is_err());
    }
}
