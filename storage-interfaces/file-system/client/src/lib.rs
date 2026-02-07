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
    compute_cid, Cid, DirectoryEntry, DirectoryNode, EntryType, FileManifest, FileSystemError,
};
use sp_core::H256;
use std::collections::HashMap;
use storage_client::StorageClient;
use thiserror::Error;

pub use file_system_primitives::DriveId;
pub use substrate::SubstrateClient;

/// File system client errors
#[derive(Debug, Error)]
pub enum FsClientError {
    #[error("File system error: {0}")]
    FileSystem(#[from] FileSystemError),

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
            .create_drive_on_chain(name, max_capacity, storage_period, payment, min_providers, strategy)
            .await?;

        // The root CID will be zero initially (empty drive)
        self.root_cache.insert(drive_id, Cid::zero());

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
        let mut chunks = Vec::new();

        for (i, chunk_data) in data.chunks(CHUNK_SIZE).enumerate() {
            let chunk_cid = compute_cid(chunk_data);
            self.upload_blob(bucket_id, chunk_data).await?;

            chunks.push(file_system_primitives::FileChunk {
                cid: Self::cid_to_string(chunk_cid),
                sequence: i as u32,
            });
        }

        // Create FileManifest
        let manifest = FileManifest {
            drive_id: drive_id.to_string(),
            mime_type: Self::guess_mime_type(file_name),
            total_size: data.len() as u64,
            chunks,
            encryption_params: String::new(),
        };

        let manifest_bytes = manifest.to_bytes()?;
        let file_cid = compute_cid(&manifest_bytes);
        self.upload_blob(bucket_id, &manifest_bytes).await?;

        // Update parent directory
        self.add_entry_to_directory(
            drive_id,
            &parent_path,
            file_name,
            file_cid,
            data.len() as u64,
            EntryType::File,
            bucket_id,
        )
        .await?;

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
        let manifest = FileManifest::from_bytes(&manifest_bytes)?;

        // Validate it's a file
        if manifest.chunks.is_empty() {
            return Err(FsClientError::NotAFile(path.to_string()));
        }

        // Fetch and reassemble chunks
        let mut file_data = Vec::with_capacity(manifest.total_size as usize);

        for chunk in manifest.chunks.iter() {
            let chunk_cid = Self::string_to_cid(&chunk.cid)?;
            let chunk_data = self.fetch_blob(chunk_cid).await?;
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
        let dir_node = DirectoryNode::from_bytes(&dir_bytes)?;

        Ok(dir_node.children)
    }

    /// Create a directory
    pub async fn create_directory(
        &mut self,
        drive_id: DriveId,
        path: &str,
        bucket_id: u64,
    ) -> Result<()> {
        let (parent_path, dir_name) = Self::split_path(path)?;

        // Create empty directory
        let new_dir = DirectoryNode::new_empty(dir_name.to_string());
        let new_dir_cid = new_dir.compute_cid()?;
        let new_dir_bytes = new_dir.to_bytes()?;

        self.upload_blob(bucket_id, &new_dir_bytes).await?;

        // Add to parent directory
        self.add_entry_to_directory(
            drive_id,
            &parent_path,
            dir_name,
            new_dir_cid,
            0,
            EntryType::Directory,
            bucket_id,
        )
        .await?;

        Ok(())
    }

    /// Get the root CID of a drive
    pub async fn get_root_cid(&mut self, drive_id: DriveId) -> Result<Cid> {
        // Check cache first
        if let Some(cid) = self.root_cache.get(&drive_id) {
            return Ok(*cid);
        }

        // Query on-chain
        let cid = self.query_drive_root_cid(drive_id).await?;
        self.root_cache.insert(drive_id, cid);

        Ok(cid)
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
            let dir_node = DirectoryNode::from_bytes(&dir_bytes)?;

            // Find child entry
            let entry = dir_node
                .children
                .iter()
                .find(|e| e.name == component)
                .ok_or_else(|| FsClientError::PathNotFound(path.to_string()))?;

            current_cid = Self::string_to_cid(&entry.cid)?;
        }

        Ok(current_cid)
    }

    /// Add an entry to a directory and update the DAG up to root
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
        let mut parent_node = DirectoryNode::from_bytes(&parent_bytes)?;

        // Check if entry already exists
        if parent_node.children.iter().any(|e| e.name == name) {
            return Err(FsClientError::EntryExists(name.to_string()));
        }

        // Add new entry
        parent_node.children.push(DirectoryEntry {
            name: name.to_string(),
            r#type: entry_type.into(),
            cid: Self::cid_to_string(cid),
            size,
            mtime: Self::current_timestamp(),
        });

        // Upload updated parent
        let new_parent_bytes = parent_node.to_bytes()?;
        let new_parent_cid = compute_cid(&new_parent_bytes);
        self.upload_blob(bucket_id, &new_parent_bytes).await?;

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
        let mut parent_node = DirectoryNode::from_bytes(&parent_bytes)?;

        // Update child entry
        for entry in &mut parent_node.children {
            if entry.name == *child_name {
                entry.cid = Self::cid_to_string(new_child_cid);
                entry.mtime = Self::current_timestamp();
                break;
            }
        }

        // Upload updated parent
        let new_parent_bytes = parent_node.to_bytes()?;
        let new_parent_cid = compute_cid(&new_parent_bytes);
        self.upload_blob(bucket_id, &new_parent_bytes).await?;

        // Recurse to grandparent (box the future to avoid infinite size)
        Box::pin(self.update_ancestors(drive_id, parent_path, new_parent_cid, bucket_id)).await
    }

    /// Upload a blob to Layer 0 storage
    async fn upload_blob(&self, bucket_id: u64, data: &[u8]) -> Result<()> {
        use storage_client::ChunkingStrategy;

        // Upload data using default chunking strategy
        let _data_root = self
            .storage_client
            .upload(bucket_id, data, ChunkingStrategy::default())
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))?;

        // Note: In production, track data_root -> cid mapping
        // Provider stores data by content hash
        Ok(())
    }

    /// Fetch a blob from Layer 0 storage by CID
    async fn fetch_blob(&self, cid: Cid) -> Result<Vec<u8>> {
        // Use the read API with CID as data root
        // Note: This assumes provider maps CID to stored data
        self.storage_client
            .read(&cid, 0, u64::MAX)
            .await
            .map_err(|e| FsClientError::StorageClient(e.to_string()))
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
        let parent = if last_slash == 0 { "/" } else { &path[..last_slash] };
        let name = &path[last_slash + 1..];

        if name.is_empty() {
            return Err(FsClientError::InvalidPath("Empty name".to_string()));
        }

        Ok((parent, name))
    }

    fn cid_to_string(cid: Cid) -> String {
        format!("0x{}", hex::encode(cid.as_bytes()))
    }

    fn string_to_cid(s: &str) -> Result<Cid> {
        let hex_str = s.strip_prefix("0x").unwrap_or(s);
        let bytes = hex::decode(hex_str)
            .map_err(|e| FsClientError::Serialization(format!("Invalid hex: {}", e)))?;

        if bytes.len() != 32 {
            return Err(FsClientError::Serialization(
                "CID must be 32 bytes".to_string(),
            ));
        }

        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(H256::from(hash))
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
            .map_err(|e| FsClientError::Blockchain(format!("Failed to submit tx: {}", e)))?;

        // Wait for finalization and extract drive_id from event
        while let Some(event) = progress.next().await {
            let event = event
                .map_err(|e| FsClientError::Blockchain(format!("Transaction error: {}", e)))?;

            if let Some(finalized) = event.as_finalized() {
                // Fetch events from the finalized block
                let events = finalized
                    .fetch_events()
                    .await
                    .map_err(|e| FsClientError::Blockchain(format!("Failed to fetch events: {}", e)))?;

                // Find DriveCreated or DriveCreatedOnBucket event
                for ev in events.iter() {
                    let ev = ev.map_err(|e| {
                        FsClientError::Blockchain(format!("Event decode error: {}", e))
                    })?;

                    // Check if this is a DriveRegistry event
                    if ev.pallet_name() == "DriveRegistry" {
                        // Try to decode as dynamic value
                        if let Ok(value) = ev.field_values() {
                            // Extract drive_id from first field (all drive events have drive_id as first field)
                            if let Some(drive_id_value) = value.at(0) {
                                if let Some(drive_id) = drive_id_value.as_u128() {
                                    log::info!("Drive created with ID: {}", drive_id);
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
            .map_err(|e| FsClientError::Blockchain(format!("Failed to submit tx: {}", e)))?;

        // Wait for finalization
        while let Some(event) = progress.next().await {
            let event = event
                .map_err(|e| FsClientError::Blockchain(format!("Transaction error: {}", e)))?;

            if event.as_finalized().is_some() {
                log::info!("Root CID updated for drive {}", drive_id);
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
            .map_err(|e| FsClientError::Blockchain(format!("Storage query failed: {}", e)))?;

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
            .map_err(|e| FsClientError::Blockchain(format!("Storage fetch failed: {}", e)))?;

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

    #[test]
    fn test_cid_conversion() {
        let cid = H256::from([1u8; 32]);
        let s = FileSystemClient::cid_to_string(cid);
        let cid2 = FileSystemClient::string_to_cid(&s).unwrap();
        assert_eq!(cid, cid2);
    }
}
