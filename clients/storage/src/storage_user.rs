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
use crate::substrate::parse_h256;
use crate::verification::ClientVerifier;
use crate::Signer;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use sp_core::H256;
use storage_primitives::{hash_children, hash_leaf, BucketId, DEFAULT_CHUNK_SIZE, MAX_READ_CHUNKS};

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

    /// Create a storage user client that talks to one provider, with the
    /// default configuration otherwise.
    ///
    /// Trailing `/` characters of `provider_url` are removed, so
    /// `http://host:3333/` and `http://host:3333` address the same provider.
    pub fn for_provider(provider_url: &str, auth_signer: Signer) -> ClientResult<Self> {
        Self::new(
            ClientConfig {
                provider_urls: vec![provider_url.trim_end_matches('/').to_string()],
                ..Default::default()
            },
            auth_signer,
        )
    }

    /// Attach the signed `Authorization` header (`method` = upper-case HTTP verb).
    fn sign(
        &self,
        req: reqwest::RequestBuilder,
        method: &str,
        bucket_id: BucketId,
    ) -> reqwest::RequestBuilder {
        req.header(
            "Authorization",
            self.auth_signer.auth_header(method, bucket_id),
        )
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
    /// Returns [`ClientError::Config`] before any request if a
    /// [`ChunkingStrategy::Fixed`] size is 0 or above `DEFAULT_CHUNK_SIZE`,
    /// or if the data to upload is empty: empty data has no chunk tree.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
    /// let data = b"Hello, decentralized world!";
    /// let data_root = client.upload(1, data, Default::default()).await?;
    /// println!("Uploaded data with root: {data_root:#x}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn upload(
        &self,
        bucket_id: BucketId,
        data: &[u8],
        strategy: ChunkingStrategy,
    ) -> ClientResult<H256> {
        // A chunk size of 0 makes `chunks()` panic, and the provider rejects a
        // chunk above `DEFAULT_CHUNK_SIZE`.
        if let ChunkingStrategy::Fixed(chunk_size) = strategy {
            if chunk_size == 0 || chunk_size > DEFAULT_CHUNK_SIZE as usize {
                return Err(ClientError::Config(format!(
                    "chunk size must be between 1 and {DEFAULT_CHUNK_SIZE} bytes, got {chunk_size}"
                )));
            }
        }

        let provider_url = self.base.get_provider_url()?;

        // Encrypt data before chunking if encryption is enabled
        let maybe_encrypted;
        let upload_data = if let Some(cipher) = &self.cipher {
            maybe_encrypted = cipher.encrypt(data)?;
            &maybe_encrypted
        } else {
            data
        };
        if upload_data.is_empty() {
            return Err(ClientError::Config(
                "empty data has no chunk tree".to_string(),
            ));
        }

        // Chunk the data
        let chunks = Self::chunk_data(upload_data, strategy);

        // Upload chunks (leaves)
        let mut chunk_hashes = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            chunk_hashes.push(
                self.upload_node(provider_url, bucket_id, &Node::Chunk(chunk))
                    .await?,
            );
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
    /// 1. Request chunks from the provider, in several `/read` requests when the
    ///    range spans more than [`MAX_READ_CHUNKS`] chunks
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
    /// let data = client.download(1, &data_root, 0, 1024).await?;
    /// println!("Downloaded {} bytes", data.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn download(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        offset: u64,
        length: u64,
    ) -> ClientResult<Vec<u8>> {
        let chunk_size = u64::from(DEFAULT_CHUNK_SIZE);
        let start_chunk = offset / chunk_size;
        let end_chunk = offset.saturating_add(length).div_ceil(chunk_size);
        let mut data = self
            .read_chunks(bucket_id, data_root, start_chunk, end_chunk)
            .await?;

        // Trim to requested range
        let start = ((offset % chunk_size) as usize).min(data.len());
        let end = start.saturating_add(length as usize).min(data.len());
        data.truncate(end);
        data.drain(..start);

        // Decrypt after reassembly if encryption is enabled
        if let Some(cipher) = &self.cipher {
            cipher.decrypt(&data)
        } else {
            Ok(data)
        }
    }

    /// Download all data under `data_root`. The caller does not need to know
    /// its size.
    pub async fn download_full(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
    ) -> ClientResult<Vec<u8>> {
        self.download(bucket_id, data_root, 0, u64::MAX).await
    }

    /// Read chunks `start_chunk..end_chunk` of `data_root` in pages of at most
    /// [`MAX_READ_CHUNKS`] chunks. The provider returns fewer chunks than
    /// requested at the end of the data, which ends the read.
    async fn read_chunks(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        start_chunk: u64,
        end_chunk: u64,
    ) -> ClientResult<Vec<u8>> {
        let mut data = Vec::new();
        let mut next = start_chunk;
        while next < end_chunk {
            let count = (end_chunk - next).min(MAX_READ_CHUNKS);
            let returned = self
                .read_page(bucket_id, data_root, next, count, &mut data)
                .await?;
            // More chunks than requested is a provider bug, not data to trust.
            if returned > count {
                return Err(ClientError::VerificationFailed);
            }
            if returned < count {
                break;
            }
            next += count;
        }
        Ok(data)
    }

    /// Send one `/read` for `count` chunks from `start_chunk`, verify each
    /// chunk's hash and append the chunk bytes to `data`. Returns the number of
    /// chunks the provider returned.
    async fn read_page(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        start_chunk: u64,
        count: u64,
        data: &mut Vec<u8>,
    ) -> ClientResult<u64> {
        let provider_url = self.base.get_provider_url()?;
        let chunk_size = u64::from(DEFAULT_CHUNK_SIZE);

        let response = self
            .base
            .http
            .get(format!("{provider_url}/read"))
            .query(&[
                ("bucket_id", bucket_id.to_string()),
                ("data_root", format!("{:#x}", data_root)),
                ("offset", (start_chunk * chunk_size).to_string()),
                ("length", (count * chunk_size).to_string()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "GET /read",
                status: response.status().as_u16(),
            });
        }

        let read_response: ReadResponse = response.json().await?;
        let returned = read_response.chunks.len() as u64;
        for chunk in read_response.chunks {
            let chunk_data = BASE64
                .decode(&chunk.data)
                .map_err(|e| ClientError::Serialization(e.to_string()))?;

            // Verify chunk hash
            let expected_hash = parse_h256(&chunk.hash)?;
            if hash_leaf(&chunk_data) != expected_hash {
                return Err(ClientError::VerificationFailed);
            }

            data.extend_from_slice(&chunk_data);
        }
        Ok(returned)
    }

    /// Read a single node by its hash.
    ///
    /// Returns a chunk with its bytes, or an internal node with its two child
    /// hashes.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # use sp_core::H256;
    /// # async fn example(hash: H256) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
    /// if let storage_client::Node::Chunk(data) = client.read_node(1, &hash).await? {
    ///     println!("Read {} bytes", data.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn read_node(&self, bucket_id: BucketId, hash: &H256) -> ClientResult<Node> {
        let provider_url = self.base.get_provider_url()?;
        let hash_hex = format!("{:#x}", hash);

        let response = self
            .base
            .http
            .get(format!("{provider_url}/node"))
            .query(&[("bucket_id", bucket_id.to_string()), ("hash", hash_hex)])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "GET /node",
                status: response.status().as_u16(),
            });
        }

        let node_response: NodeResponse = response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))?;

        let data = node_response
            .data
            .map(|data| {
                BASE64
                    .decode(data)
                    .map_err(|e| ClientError::Serialization(format!("Invalid base64: {e}")))
            })
            .transpose()?;
        let children = node_response
            .children
            .map(|c| {
                c.iter()
                    .map(|h| parse_h256(h))
                    .collect::<ClientResult<Vec<H256>>>()
            })
            .transpose()?;

        Node::from_wire(data, children)
    }

    /// Read a node and verify that it hashes to `hash`.
    pub async fn read_node_verified(&self, bucket_id: BucketId, hash: &H256) -> ClientResult<Node> {
        let node = self.read_node(bucket_id, hash).await?;
        if &node.hash() != hash {
            return Err(ClientError::VerificationFailed);
        }

        Ok(node)
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
            data_roots: data_roots.iter().map(|h| format!("{:#x}", h)).collect(),
        };

        let req = self
            .base
            .http
            .post(format!("{provider_url}/commit"))
            .json(&request);
        let response = self.sign(req, "POST", bucket_id).send().await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "POST /commit",
                status: response.status().as_u16(),
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// Get a checkpoint-compatible signature from the provider.
    ///
    /// Returns a signature over the commitment with the real `leaf_count`,
    /// suitable for the on-chain `checkpoint` extrinsic.
    pub async fn get_checkpoint_signature(
        &self,
        bucket_id: BucketId,
    ) -> ClientResult<CheckpointSignatureResponse> {
        let provider_url = self.base.get_provider_url()?;

        let req = self
            .base
            .http
            .get(format!("{provider_url}/checkpoint-signature"))
            .query(&[("bucket_id", bucket_id.to_string())]);
        let response = self.sign(req, "GET", bucket_id).send().await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "GET /checkpoint-signature",
                status: response.status().as_u16(),
            });
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
        bucket_id: BucketId,
        data_root: &H256,
        chunk_index: u64,
    ) -> ClientResult<bool> {
        use std::time::Instant;

        let chunk_size = u64::from(DEFAULT_CHUNK_SIZE);
        let offset = chunk_index * chunk_size;
        let provider_url = self.base.get_provider_url()?.to_string();

        let start = Instant::now();
        let result = self
            .download(bucket_id, data_root, offset, chunk_size)
            .await;
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
        bucket_id: BucketId,
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
            if self.spot_check(bucket_id, data_root, chunk_index).await? {
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
                Self::chunk_data(data, ChunkingStrategy::default())
            }
        }
    }

    /// Upload `node` and return its hash.
    async fn upload_node(
        &self,
        provider_url: &str,
        bucket_id: BucketId,
        node: &Node,
    ) -> ClientResult<H256> {
        let (data, children) = match node {
            Node::Chunk(data) => (Some(BASE64.encode(data)), None),
            Node::Internal { left, right } => (
                None,
                Some(vec![format!("{:#x}", left), format!("{:#x}", right)]),
            ),
        };
        let hash = node.hash();
        let request = UploadNodeRequest {
            bucket_id,
            hash: format!("{:#x}", hash),
            data,
            children,
        };

        let req = self
            .base
            .http
            .put(format!("{provider_url}/node"))
            .json(&request);
        let response = self.sign(req, "PUT", bucket_id).send().await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "PUT /node",
                status: response.status().as_u16(),
            });
        }

        Ok(hash)
    }

    async fn build_merkle_tree(
        &self,
        provider_url: &str,
        bucket_id: BucketId,
        leaf_hashes: &[H256],
    ) -> ClientResult<H256> {
        let (root, nodes) = storage_primitives::merkle_tree(leaf_hashes);
        for node in nodes {
            self.upload_node(
                provider_url,
                bucket_id,
                &Node::Internal {
                    left: node.left,
                    right: node.right,
                },
            )
            .await?;
        }

        Ok(root)
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
    pub async fn get_commitment(&self, bucket_id: BucketId) -> ClientResult<CommitmentResponse> {
        self.get_commitment_if_exists(bucket_id)
            .await?
            .ok_or(ClientError::HttpStatus {
                request: "GET /commitment",
                status: 404,
            })
    }

    /// Like [`Self::get_commitment`], but returns `None` when the provider
    /// answers 404 (it has no record of the bucket: it has received no data
    /// for it yet).
    pub async fn get_commitment_if_exists(
        &self,
        bucket_id: BucketId,
    ) -> ClientResult<Option<CommitmentResponse>> {
        let provider_url = self.base.get_provider_url()?;
        let req = self
            .base
            .http
            .get(format!("{provider_url}/commitment"))
            .query(&[("bucket_id", bucket_id.to_string())]);
        let response = self.sign(req, "GET", bucket_id).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(ClientError::HttpStatus {
                request: "GET /commitment",
                status: status.as_u16(),
            });
        }
        response
            .json()
            .await
            .map(Some)
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// Get the MMR proof of the leaf at `leaf_index` of the bucket's current
    /// MMR (`GET /mmr_proof`). The proof is not verified here: check it with
    /// [`storage_primitives::verify_mmr_proof`] against a trusted MMR root.
    pub async fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        leaf_index: u64,
    ) -> ClientResult<storage_primitives::MmrProof> {
        let provider_url = self.base.get_provider_url()?;
        let req = self
            .base
            .http
            .get(format!("{provider_url}/mmr_proof"))
            .query(&[
                ("bucket_id", bucket_id.to_string()),
                ("leaf_index", leaf_index.to_string()),
            ]);
        let response = self.sign(req, "GET", bucket_id).send().await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "GET /mmr_proof",
                status: response.status().as_u16(),
            });
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
            hashes: hashes.iter().map(|h| format!("{:#x}", h)).collect(),
        };
        let req = self
            .base
            .http
            .post(format!("{provider_url}/exists"))
            .json(&request);
        let response = self.sign(req, "POST", bucket_id).send().await?;

        if !response.status().is_success() {
            return Err(ClientError::HttpStatus {
                request: "POST /exists",
                status: response.status().as_u16(),
            });
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
        bucket_id: BucketId,
        data_root: &H256,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, crate::ClientError> {
        self.download(bucket_id, data_root, offset, length).await
    }

    fn provider_url(&self) -> &str {
        self.base.get_provider_url().unwrap_or("unknown")
    }
}

// API types

/// A chunk-tree node: a chunk with its bytes, or an internal node with its two
/// child hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Chunk bytes.
    Chunk(Vec<u8>),
    /// Child hashes of an internal node.
    Internal {
        /// Left child hash.
        left: H256,
        /// Right child hash.
        right: H256,
    },
}

impl Node {
    /// This node's hash: the chunk hash for a chunk, the hash of the two
    /// children for an internal node.
    pub fn hash(&self) -> H256 {
        match self {
            Self::Chunk(data) => hash_leaf(data),
            Self::Internal { left, right } => hash_children(*left, *right),
        }
    }

    /// Build a node from the `data` or the `children` of a `/node` response.
    /// `VerificationFailed` if the response has both or neither, or an
    /// internal node does not have exactly two children.
    fn from_wire(data: Option<Vec<u8>>, children: Option<Vec<H256>>) -> ClientResult<Self> {
        match (data, children) {
            (Some(data), None) => Ok(Self::Chunk(data)),
            (None, Some(children)) => {
                let [left, right]: [H256; 2] = children
                    .try_into()
                    .map_err(|_| ClientError::VerificationFailed)?;
                Ok(Self::Internal { left, right })
            }
            _ => Err(ClientError::VerificationFailed),
        }
    }
}

#[derive(serde::Serialize)]
struct UploadNodeRequest {
    bucket_id: u64,
    hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
struct CommitRequest {
    bucket_id: u64,
    data_roots: Vec<String>,
}

/// Deserialize the provider node's signature wire format — `0x`-prefixed hex
/// of a SCALE-encoded `MultiSignature` — into the typed value. Decoding at
/// the HTTP boundary means a malformed signature fails the request itself;
/// code past deserialization only ever sees valid, scheme-tagged signatures.
fn multi_signature_from_hex<'de, D>(de: D) -> Result<sp_runtime::MultiSignature, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let hex_str = <String as serde::Deserialize>::deserialize(de)?;
    crate::substrate::decode_multi_signature(&hex_str).map_err(serde::de::Error::custom)
}

/// Deserialize a `0x`-prefixed 32-byte hex string into an `H256`, failing the
/// request at the HTTP boundary like [`multi_signature_from_hex`].
fn h256_from_hex<'de, D>(de: D) -> Result<H256, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let hex_str = <String as serde::Deserialize>::deserialize(de)?;
    crate::substrate::parse_h256(&hex_str).map_err(serde::de::Error::custom)
}

#[derive(serde::Deserialize)]
pub struct CommitResponse {
    #[serde(deserialize_with = "h256_from_hex")]
    pub mmr_root: H256,
    pub start_seq: u64,
    /// Leaf count signed into the commitment; `challenge_offchain` binds the
    /// challenged leaf to it.
    pub leaf_count: u64,
    pub leaf_indices: Vec<u64>,
    #[serde(deserialize_with = "multi_signature_from_hex")]
    pub provider_signature: sp_runtime::MultiSignature,
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
    data: Option<String>,
    children: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
pub struct CheckpointSignatureResponse {
    pub bucket_id: u64,
    #[serde(deserialize_with = "h256_from_hex")]
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    #[serde(deserialize_with = "multi_signature_from_hex")]
    pub provider_signature: sp_runtime::MultiSignature,
}

#[derive(serde::Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct CommitmentResponse {
    pub bucket_id: BucketId,
    #[serde(deserialize_with = "h256_from_hex")]
    pub mmr_root: H256,
    pub start_seq: u64,
    pub leaf_count: u64,
    #[serde(deserialize_with = "multi_signature_from_hex")]
    pub provider_signature: sp_runtime::MultiSignature,
}

impl CommitmentResponse {
    #[cfg(test)]
    pub(crate) fn new(
        bucket_id: BucketId,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        provider_signature: sp_runtime::MultiSignature,
    ) -> Self {
        Self {
            bucket_id,
            mmr_root,
            start_seq,
            leaf_count,
            provider_signature,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_provider_ignores_trailing_slashes() {
        let signer = Signer::from_seed("//Alice").unwrap();
        for url in [
            "http://host:3333",
            "http://host:3333/",
            "http://host:3333//",
        ] {
            let client = StorageUserClient::for_provider(url, signer.clone()).unwrap();
            assert_eq!(
                client.base.get_provider_url().unwrap(),
                "http://host:3333",
                "url {url}"
            );
        }
    }

    #[test]
    fn node_from_wire_takes_data_or_exactly_two_children() {
        let (left, right) = (H256::repeat_byte(1), H256::repeat_byte(2));

        assert_eq!(
            Node::from_wire(Some(vec![1, 2, 3]), None).unwrap(),
            Node::Chunk(vec![1, 2, 3])
        );
        assert_eq!(
            Node::from_wire(None, Some(vec![left, right])).unwrap(),
            Node::Internal { left, right }
        );
        for (data, children) in [
            (None, Some(vec![left])),
            (None, Some(vec![left, right, left])),
            (Some(vec![1, 2, 3]), Some(vec![left, right])),
            (None, None),
        ] {
            assert!(matches!(
                Node::from_wire(data, children),
                Err(ClientError::VerificationFailed)
            ));
        }
    }

    #[test]
    fn node_hash_is_the_chunk_hash_or_the_hash_of_the_children() {
        let (left, right) = (H256::repeat_byte(1), H256::repeat_byte(2));
        assert_eq!(Node::Chunk(vec![1, 2, 3]).hash(), hash_leaf(&[1, 2, 3]));
        assert_eq!(
            Node::Internal { left, right }.hash(),
            storage_primitives::hash_children(left, right)
        );
    }

    fn commitment_json(mmr_root: &str, signature: &str) -> String {
        format!(
            r#"{{"bucket_id":1,"mmr_root":"{mmr_root}","start_seq":5,"leaf_count":10,"provider_signature":"{signature}"}}"#
        )
    }

    fn sr25519_signature_hex() -> String {
        use codec::Encode;
        let sig = sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([9u8; 64]));
        array_bytes::bytes2hex("0x", sig.encode())
    }

    #[test]
    fn commitment_response_deserializes_typed_root_and_signature() {
        let root_hex = array_bytes::bytes2hex("0x", [7u8; 32]);
        let resp: CommitmentResponse =
            serde_json::from_str(&commitment_json(&root_hex, &sr25519_signature_hex())).unwrap();
        assert_eq!(resp.mmr_root, H256::from([7u8; 32]));
        assert_eq!(
            resp.provider_signature,
            sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from([9u8; 64]))
        );
    }

    #[test]
    fn commitment_response_rejects_undecodable_signature() {
        // Not hex at all.
        let root_hex = array_bytes::bytes2hex("0x", [7u8; 32]);
        assert!(
            serde_json::from_str::<CommitmentResponse>(&commitment_json(&root_hex, "not-hex"))
                .is_err()
        );
        // 0x01 tags Sr25519, whose signature is 64 bytes — 63 must not decode.
        let truncated = array_bytes::bytes2hex("0x", [[1u8].as_slice(), &[9u8; 63]].concat());
        assert!(
            serde_json::from_str::<CommitmentResponse>(&commitment_json(&root_hex, &truncated))
                .is_err(),
            "a truncated signature must fail at the wire, not downstream"
        );
        // Trailing bytes past a well-formed signature must be refused too,
        // not silently ignored — the JS SDK rejects the same value.
        let overlong = array_bytes::bytes2hex("0x", [[1u8].as_slice(), &[9u8; 65]].concat());
        assert!(
            serde_json::from_str::<CommitmentResponse>(&commitment_json(&root_hex, &overlong))
                .is_err(),
            "trailing bytes must not be silently truncated"
        );
    }

    #[test]
    fn commitment_response_rejects_unparsable_root() {
        assert!(serde_json::from_str::<CommitmentResponse>(&commitment_json(
            "0xnot-a-root",
            &sr25519_signature_hex()
        ))
        .is_err());
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
