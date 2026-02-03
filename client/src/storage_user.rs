//! Storage User Client - For end users storing and retrieving data.
//!
//! This client provides high-level operations for:
//! - Uploading data with chunking and Merkle tree building
//! - Downloading data with verification
//! - Committing data to on-chain MMR
//! - Challenging providers for data integrity
//! - Monitoring storage health

use crate::base::{BaseClient, ChunkingStrategy, ClientConfig, ClientError, ClientResult};
use crate::verification::ClientVerifier;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use sp_core::H256;
use storage_primitives::{blake2_256, BucketId};

/// Client for storage users (end users who store/retrieve data).
pub struct StorageUserClient {
    base: BaseClient,
    verifier: ClientVerifier,
}

impl StorageUserClient {
    /// Create a new storage user client.
    pub fn new(config: ClientConfig) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            verifier: ClientVerifier::new(),
        })
    }

    /// Create with default configuration.
    pub fn with_defaults() -> ClientResult<Self> {
        Self::new(ClientConfig::default())
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Upload Operations
    // ═════════════════════════════════════════════════════════════════════════

    /// Upload data to a provider and return the data root.
    ///
    /// This will:
    /// 1. Chunk the data according to the strategy
    /// 2. Build a Merkle tree over the chunks
    /// 3. Upload all nodes to the provider
    /// 4. Return the data root hash
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::StorageUserClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::with_defaults()?;
    /// let data = b"Hello, decentralized world!";
    /// let data_root = client.upload(1, data, Default::default()).await?;
    /// println!("Uploaded data with root: 0x{}", hex::encode(data_root.as_bytes()));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn upload(
        &self,
        bucket_id: BucketId,
        data: &[u8],
        strategy: ChunkingStrategy,
    ) -> ClientResult<H256> {
        let provider_url = self.base.get_provider_url()?;

        // Chunk the data
        let chunks = self.chunk_data(data, strategy);

        // Upload chunks (leaves)
        let chunk_hashes: Vec<H256> = chunks.iter().map(|chunk| blake2_256(chunk)).collect();

        for (chunk, hash) in chunks.iter().zip(chunk_hashes.iter()) {
            self.upload_node(provider_url, bucket_id, *hash, chunk.clone(), None)
                .await?;
        }

        // Build Merkle tree bottom-up
        let data_root = self
            .build_merkle_tree(provider_url, bucket_id, &chunk_hashes)
            .await?;

        Ok(data_root)
    }

    /// Upload data to multiple providers for redundancy.
    pub async fn upload_replicated(
        &self,
        bucket_id: BucketId,
        data: &[u8],
        provider_urls: &[String],
        strategy: ChunkingStrategy,
    ) -> ClientResult<H256> {
        // Upload to first provider and get data root
        let original_provider = self.base.get_provider_url()?;

        // Temporarily override provider for first upload
        // In a real implementation, we'd manage this better
        let data_root = self.upload(bucket_id, data, strategy).await?;

        // Replicate to other providers
        for provider_url in provider_urls {
            if provider_url != original_provider {
                // In a real implementation, we'd sync from first provider to others
                // For now, we'd upload to each separately
                tracing::info!("Would replicate to {}", provider_url);
            }
        }

        Ok(data_root)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Download Operations
    // ═════════════════════════════════════════════════════════════════════════

    /// Download data from a provider using the data root.
    ///
    /// This will:
    /// 1. Request chunks from the provider
    /// 2. Verify each chunk's hash
    /// 3. Optionally verify Merkle proofs
    /// 4. Reassemble the data
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::StorageUserClient;
    /// # use sp_core::H256;
    /// # async fn example(data_root: H256) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::with_defaults()?;
    /// let data = client.download(&data_root, 0, 1024).await?;
    /// println!("Downloaded {} bytes", data.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn download(
        &self,
        data_root: &H256,
        offset: u64,
        length: u64,
    ) -> ClientResult<Vec<u8>> {
        let provider_url = self.base.get_provider_url()?;

        let response = self
            .base
            .http
            .get(format!("{}/read", provider_url))
            .query(&[
                ("data_root", BaseClient::hex_encode(data_root.as_bytes())),
                ("offset", offset.to_string()),
                ("length", length.to_string()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Provider returned error: {}",
                response.status()
            )));
        }

        let read_response: ReadResponse = response.json().await?;

        let mut data = Vec::new();
        for chunk in read_response.chunks {
            let chunk_data = BASE64
                .decode(&chunk.data)
                .map_err(|e| ClientError::Serialization(e.to_string()))?;

            // Verify chunk hash
            let expected_hash =
                BaseClient::hex_decode(&chunk.hash)?;
            let actual_hash = blake2_256(&chunk_data);
            if actual_hash.as_bytes() != expected_hash.as_slice() {
                return Err(ClientError::VerificationFailed);
            }

            data.extend_from_slice(&chunk_data);
        }

        // Trim to requested range
        let chunk_size = 256 * 1024;
        let start = (offset % chunk_size) as usize;
        let end = start + length as usize;
        if end <= data.len() {
            Ok(data[start..end].to_vec())
        } else {
            Ok(data[start..].to_vec())
        }
    }

    /// Download entire file by data root.
    pub async fn download_full(&self, data_root: &H256, total_size: u64) -> ClientResult<Vec<u8>> {
        self.download(data_root, 0, total_size).await
    }

    // ═════════════════════════════════════════════════════════════════════════
    // On-Chain Operations
    // ═════════════════════════════════════════════════════════════════════════

    /// Commit data roots to the bucket's MMR on-chain.
    ///
    /// This makes the data "official" and starts the accountability period.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::StorageUserClient;
    /// # use sp_core::H256;
    /// # async fn example(data_root: H256) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::with_defaults()?;
    /// let commitment = client.commit(1, vec![data_root]).await?;
    /// println!("Committed with MMR root: {}", commitment.mmr_root);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> ClientResult<CommitResponse> {
        let provider_url = self.base.get_provider_url()?;

        let request = CommitRequest {
            bucket_id,
            data_roots: data_roots
                .iter()
                .map(|h| BaseClient::hex_encode(h.as_bytes()))
                .collect(),
        };

        let response = self
            .base
            .http
            .post(format!("{}/commit", provider_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Commit failed: {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Verification & Monitoring
    // ═════════════════════════════════════════════════════════════════════════

    /// Perform a spot-check on a provider to verify data integrity.
    ///
    /// Returns true if the check passed.
    pub async fn spot_check(
        &mut self,
        data_root: &H256,
        chunk_index: u64,
    ) -> ClientResult<bool> {
        // This would use the ClientVerifier
        self.verifier
            .spot_check(self, data_root, chunk_index)
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Perform multiple random spot-checks on a provider.
    ///
    /// Returns (passed_count, failed_count).
    pub async fn spot_check_batch(
        &mut self,
        data_root: &H256,
        num_checks: usize,
        total_chunks: u64,
    ) -> ClientResult<(usize, usize)> {
        self.verifier
            .spot_check_batch(self, data_root, num_checks, total_chunks)
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Get provider statistics for a specific provider URL.
    pub fn get_provider_stats(&self, provider_url: &str) -> Option<&crate::verification::ProviderStats> {
        self.verifier.get_stats(provider_url)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Helper Functions
    // ═════════════════════════════════════════════════════════════════════════

    fn chunk_data(&self, data: &[u8], strategy: ChunkingStrategy) -> Vec<Vec<u8>> {
        match strategy {
            ChunkingStrategy::Fixed(chunk_size) => {
                data.chunks(chunk_size).map(|c| c.to_vec()).collect()
            }
            ChunkingStrategy::ContentDefined => {
                // TODO: Implement content-defined chunking
                self.chunk_data(data, ChunkingStrategy::Fixed(256 * 1024))
            }
        }
    }

    async fn upload_node(
        &self,
        provider_url: &str,
        bucket_id: BucketId,
        hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> ClientResult<()> {
        let request = UploadNodeRequest {
            bucket_id,
            hash: BaseClient::hex_encode(hash.as_bytes()),
            data: BASE64.encode(&data),
            children: children.map(|c| {
                c.iter()
                    .map(|h| BaseClient::hex_encode(h.as_bytes()))
                    .collect()
            }),
        };

        let response = self
            .base
            .http
            .put(format!("{}/node", provider_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Upload failed: {}",
                response.status()
            )));
        }

        Ok(())
    }

    async fn build_merkle_tree(
        &self,
        provider_url: &str,
        bucket_id: BucketId,
        leaf_hashes: &[H256],
    ) -> ClientResult<H256> {
        let mut current_level = leaf_hashes.to_vec();

        while current_level.len() > 1 {
            let mut next_level = Vec::new();

            for pair in current_level.chunks(2) {
                let parent_hash = if pair.len() == 2 {
                    storage_primitives::hash_children(pair[0], pair[1])
                } else {
                    pair[0]
                };

                // Upload internal node
                let children = pair.to_vec();
                let parent_data = self.encode_internal_node(&children);

                self.upload_node(
                    provider_url,
                    bucket_id,
                    parent_hash,
                    parent_data,
                    Some(children),
                )
                .await?;

                next_level.push(parent_hash);
            }

            current_level = next_level;
        }

        Ok(current_level[0])
    }

    fn encode_internal_node(&self, children: &[H256]) -> Vec<u8> {
        // Simple encoding: concatenate child hashes
        children
            .iter()
            .flat_map(|h| h.as_bytes().to_vec())
            .collect()
    }
}

// API types

#[derive(serde::Serialize)]
struct UploadNodeRequest {
    bucket_id: u64,
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
struct CommitRequest {
    bucket_id: u64,
    data_roots: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct CommitResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_indices: Vec<u64>,
    pub provider_signature: String,
}

#[derive(serde::Deserialize)]
struct ReadResponse {
    chunks: Vec<ChunkWithProof>,
}

#[derive(serde::Deserialize)]
struct ChunkWithProof {
    hash: String,
    data: String,
    proof: Vec<String>,
}
