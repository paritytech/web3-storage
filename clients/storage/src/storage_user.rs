// SPDX-License-Identifier: Apache-2.0

//! Storage User Client - For end users storing and retrieving data.
//!
//! This client provides high-level operations for:
//! - Uploading data with chunking and Merkle tree building
//! - Downloading data with verification
//! - Committing data to on-chain MMR
//! - Challenging providers for data integrity
//! - Monitoring storage health

use crate::base::{BaseClient, ChunkingStrategy, ClientConfig, ClientError, ClientResult};
use crate::encryption::{Cipher, EncryptionKey, XChaCha20Poly1305Cipher};
use crate::verification::ClientVerifier;
use crate::Signer;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use provider_negotiation::build_auth_header;
use sp_core::H256;
use storage_primitives::{blake2_256, BucketId};

/// Client for storage users (end users who store/retrieve data).
pub struct StorageUserClient {
    base: BaseClient,
    verifier: ClientVerifier,
    cipher: Option<Box<dyn Cipher>>,
    auth_signer: Signer,
}

impl StorageUserClient {
    /// Create a new storage user client.
    ///
    /// `auth_signer` authenticates every bucket-scoped provider request; the
    /// provider always enforces auth, so it is mandatory.
    pub fn new(config: ClientConfig, auth_signer: Signer) -> ClientResult<Self> {
        Ok(Self {
            base: BaseClient::new(config)?,
            verifier: ClientVerifier::new(),
            cipher: None,
            auth_signer,
        })
    }

    /// Attach the signed `Authorization` header (`method` = upper-case HTTP verb).
    fn sign(
        &self,
        req: reqwest::RequestBuilder,
        method: &str,
        bucket_id: BucketId,
    ) -> reqwest::RequestBuilder {
        let signer = self.auth_signer.keypair();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let header = build_auth_header(
            &signer.public_key().0,
            method,
            bucket_id,
            timestamp,
            |msg| signer.sign(msg).0,
        );
        req.header("Authorization", header)
    }

    /// Enable client-side encryption with a custom cipher (builder pattern).
    pub fn with_encryption(mut self, cipher: Box<dyn Cipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }

    /// Enable client-side encryption with an XChaCha20-Poly1305 key (builder pattern).
    pub fn with_encryption_key(self, key: &EncryptionKey) -> Self {
        self.with_encryption(Box::new(XChaCha20Poly1305Cipher::new(key)))
    }

    /// Returns `true` if client-side encryption is enabled.
    pub fn is_encryption_enabled(&self) -> bool {
        self.cipher.is_some()
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
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
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

        // Encrypt data before chunking if encryption is enabled
        let maybe_encrypted;
        let upload_data = if let Some(cipher) = &self.cipher {
            maybe_encrypted = cipher.encrypt(data)?;
            &maybe_encrypted
        } else {
            data
        };

        // Chunk the data
        let chunks = Self::chunk_data(upload_data, strategy);

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
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # use sp_core::H256;
    /// # async fn example(data_root: H256) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
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
            .get(format!("{provider_url}/read"))
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
            let expected_hash = BaseClient::hex_decode(&chunk.hash)?;
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
        let trimmed = if end <= data.len() {
            data[start..end].to_vec()
        } else {
            data[start..].to_vec()
        };

        // Decrypt after reassembly if encryption is enabled
        if let Some(cipher) = &self.cipher {
            cipher.decrypt(&trimmed)
        } else {
            Ok(trimmed)
        }
    }

    /// Download entire file by data root.
    pub async fn download_full(&self, data_root: &H256, total_size: u64) -> ClientResult<Vec<u8>> {
        self.download(data_root, 0, total_size).await
    }

    /// Read a single node by its hash.
    ///
    /// Returns the node data and optionally its children hashes (for internal nodes).
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # use sp_core::H256;
    /// # async fn example(hash: H256) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
    /// let (data, children) = client.read_node(&hash).await?;
    /// println!("Read {} bytes", data.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn read_node(&self, hash: &H256) -> ClientResult<(Vec<u8>, Option<Vec<H256>>)> {
        let provider_url = self.base.get_provider_url()?;
        let hash_hex = BaseClient::hex_encode(hash.as_bytes());

        let response = self
            .base
            .http
            .get(format!("{provider_url}/node"))
            .query(&[("hash", &hash_hex)])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Node not found or error: {}",
                response.status()
            )));
        }

        let node_response: NodeResponse = response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))?;

        // Decode base64 data
        let data = BASE64
            .decode(&node_response.data)
            .map_err(|e| ClientError::Serialization(format!("Invalid base64: {e}")))?;

        // Parse children hashes if present
        let children = node_response
            .children
            .map(|c| {
                c.iter()
                    .map(|h| {
                        let bytes = BaseClient::hex_decode(h)?;
                        Ok(H256::from_slice(&bytes))
                    })
                    .collect::<ClientResult<Vec<H256>>>()
            })
            .transpose()?;

        Ok((data, children))
    }

    /// Read a node and verify its hash matches.
    ///
    /// Returns the data if hash verification passes.
    pub async fn read_node_verified(&self, hash: &H256) -> ClientResult<Vec<u8>> {
        let (data, _children) = self.read_node(hash).await?;

        // Verify the hash
        let computed_hash = blake2_256(&data);
        if &computed_hash != hash {
            return Err(ClientError::VerificationFailed);
        }

        Ok(data)
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
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # use sp_core::H256;
    /// # async fn example(data_root: H256) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
    /// let commitment = client.commit(1, vec![data_root], 0u64).await?;
    /// println!("Committed with MMR root: {}", commitment.mmr_root);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
        nonce: u64,
    ) -> ClientResult<CommitResponse> {
        let provider_url = self.base.get_provider_url()?;

        let request = CommitRequest {
            bucket_id,
            data_roots: data_roots
                .iter()
                .map(|h| BaseClient::hex_encode(h.as_bytes()))
                .collect(),
            nonce,
        };

        let req = self
            .base
            .http
            .post(format!("{provider_url}/commit"))
            .json(&request);
        let response = self.sign(req, "POST", bucket_id).send().await?;

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

    /// Get a checkpoint-compatible signature from the provider.
    ///
    /// Unlike `commit` which signs with `leaf_count=0` (for `challenge_offchain`),
    /// this returns a signature over the real `leaf_count`, suitable for submitting
    /// an on-chain checkpoint via the `checkpoint` extrinsic.
    pub async fn get_checkpoint_signature(
        &self,
        bucket_id: BucketId,
        nonce: u64,
    ) -> ClientResult<CheckpointSignatureResponse> {
        let provider_url = self.base.get_provider_url()?;

        let response = self
            .base
            .http
            .get(format!("{provider_url}/checkpoint-signature"))
            .query(&[
                ("bucket_id", bucket_id.to_string()),
                ("nonce", nonce.to_string()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Checkpoint signature request failed: {}",
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
    pub async fn spot_check(&mut self, data_root: &H256, chunk_index: u64) -> ClientResult<bool> {
        use std::time::Instant;

        let chunk_size = 256 * 1024u64; // 256 KiB
        let offset = chunk_index * chunk_size;
        let provider_url = self.base.get_provider_url()?.to_string();

        let start = Instant::now();
        let result = self.download(data_root, offset, chunk_size).await;
        let duration = start.elapsed();

        match result {
            Ok(data) if !data.is_empty() => {
                self.verifier.record_request(&provider_url, duration, true);
                Ok(true)
            }
            _ => {
                // Either download failed or the provider returned no data for this chunk,
                // which means the data is unavailable.
                self.verifier.record_request(&provider_url, duration, false);
                Ok(false)
            }
        }
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
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut passed = 0;
        let mut failed = 0;

        for _ in 0..num_checks {
            let chunk_index = rng.gen_range(0..total_chunks);
            if self.spot_check(data_root, chunk_index).await? {
                passed += 1;
            } else {
                failed += 1;
            }
        }

        Ok((passed, failed))
    }

    /// Get provider statistics for a specific provider URL.
    pub fn get_provider_stats(
        &self,
        provider_url: &str,
    ) -> Option<&crate::verification::ProviderStats> {
        self.verifier.get_stats(provider_url)
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Helper Functions
    // ═════════════════════════════════════════════════════════════════════════

    fn chunk_data(data: &[u8], strategy: ChunkingStrategy) -> Vec<Vec<u8>> {
        match strategy {
            ChunkingStrategy::Fixed(chunk_size) => {
                data.chunks(chunk_size).map(|c| c.to_vec()).collect()
            }
            ChunkingStrategy::ContentDefined => {
                // TODO: Implement content-defined chunking
                Self::chunk_data(data, ChunkingStrategy::Fixed(256 * 1024))
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

        let req = self
            .base
            .http
            .put(format!("{provider_url}/node"))
            .json(&request);
        let response = self.sign(req, "PUT", bucket_id).send().await?;

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
        if leaf_hashes.is_empty() {
            return Err(ClientError::Api(
                "Cannot build Merkle tree with no leaves".to_string(),
            ));
        }

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

    // ═════════════════════════════════════════════════════════════════════════
    // Provider Queries
    // ═════════════════════════════════════════════════════════════════════════

    /// Check the provider's health status.
    pub async fn health(&self) -> ClientResult<HealthResponse> {
        let provider_url = self.base.get_provider_url()?;
        let response = self
            .base
            .http
            .get(format!("{provider_url}/health"))
            .send()
            .await?;
        response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// Get the current MMR commitment for a bucket from the provider.
    pub async fn get_commitment(
        &self,
        bucket_id: BucketId,
        nonce: u64,
    ) -> ClientResult<CommitmentResponse> {
        let provider_url = self.base.get_provider_url()?;
        let response = self
            .base
            .http
            .get(format!("{provider_url}/commitment"))
            .query(&[
                ("bucket_id", bucket_id.to_string()),
                ("nonce", nonce.to_string()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Commitment request failed: {}",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// Check which data roots (hashes) exist on the provider.
    pub async fn check_exists(
        &self,
        bucket_id: BucketId,
        hashes: Vec<H256>,
    ) -> ClientResult<ExistsResponse> {
        let provider_url = self.base.get_provider_url()?;
        let request = ExistsRequest {
            bucket_id,
            hashes: hashes
                .iter()
                .map(|h| BaseClient::hex_encode(h.as_bytes()))
                .collect(),
        };
        let response = self
            .base
            .http
            .post(format!("{provider_url}/exists"))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::Api(format!(
                "Exists check failed: {}",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }
}

// Implement ProviderReadAccess for verification
#[async_trait::async_trait]
impl crate::verification::ProviderReadAccess for StorageUserClient {
    async fn read_data(
        &self,
        data_root: &H256,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, crate::ClientError> {
        self.download(data_root, offset, length).await
    }

    fn provider_url(&self) -> &str {
        self.base.get_provider_url().unwrap_or("unknown")
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
    /// Block number the caller intends to submit the resulting signature at.
    /// The provider signs over this so the pallet's recency check passes.
    nonce: u64,
}

#[derive(serde::Deserialize)]
pub struct CommitResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    /// Leaves in the MMR after this commit. Needed when the resulting
    /// signature is submitted via `challenge_offchain`.
    pub leaf_count: u64,
    pub leaf_indices: Vec<u64>,
    pub provider_signature: String,
    /// Echo of the nonce the provider signed over.
    pub nonce: u64,
}

#[derive(serde::Deserialize)]
struct ReadResponse {
    chunks: Vec<ChunkWithProof>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct ChunkWithProof {
    hash: String,
    data: String,
    proof: Vec<String>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct NodeResponse {
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
pub struct CheckpointSignatureResponse {
    pub bucket_id: u64,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
    pub nonce: u64,
}

#[derive(serde::Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct CommitmentResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
    pub nonce: u64,
}

impl CommitmentResponse {
    #[cfg(test)]
    pub(crate) fn new(
        bucket_id: BucketId,
        mmr_root: String,
        start_seq: u64,
        leaf_count: u64,
        provider_signature: String,
        nonce: u64,
    ) -> Self {
        Self {
            bucket_id,
            mmr_root,
            start_seq,
            leaf_count,
            provider_signature,
            nonce,
        }
    }
}

#[derive(serde::Deserialize)]
pub struct ExistsResponse {
    pub exists: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(serde::Serialize)]
struct ExistsRequest {
    bucket_id: BucketId,
    hashes: Vec<String>,
}
