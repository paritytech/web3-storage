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
use storage_primitives::{
    hash_children, hash_leaf, merkle_root, verify_merkle_proof, BucketId, DEFAULT_CHUNK_SIZE,
};

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

    /// Download `offset..offset + length` of the data under `data_root`,
    /// whose stored size is `size` bytes (with encryption, the size of the
    /// encrypted data).
    ///
    /// This will:
    /// 1. Request chunks from the provider, in several `/read` requests when the
    ///    provider limits the response size
    /// 2. Verify each chunk's hash and its proof against `data_root`, at its
    ///    position in a tree of `size` bytes
    /// 3. Reassemble the data
    ///
    /// The data must use `DEFAULT_CHUNK_SIZE` chunks, as committed data does.
    /// For data of unknown size or chunk size, use [`Self::download_full`].
    /// A range of no bytes, or one that starts at or past `size`, returns no
    /// bytes without a request.
    ///
    /// # Example
    /// ```no_run
    /// # use storage_client::{ClientConfig, Signer, StorageUserClient};
    /// # use sp_core::H256;
    /// # async fn example(data_root: H256, size: u64) -> Result<(), Box<dyn std::error::Error>> {
    /// let client = StorageUserClient::new(ClientConfig::default(), Signer::from_seed("//Alice")?)?;
    /// let data = client.download(1, &data_root, size, 0, 1024).await?;
    /// println!("Downloaded {} bytes", data.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn download(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        size: u64,
        offset: u64,
        length: u64,
    ) -> ClientResult<Vec<u8>> {
        if length == 0 || offset >= size {
            return Ok(Vec::new());
        }
        let chunk_count = size.div_ceil(u64::from(DEFAULT_CHUNK_SIZE));
        self.download_range(bucket_id, data_root, offset, length, Some(chunk_count))
            .await
    }

    /// Read `offset..offset + length` of `data_root` in `DEFAULT_CHUNK_SIZE`
    /// chunks, trim it to the range and decrypt it. Without `chunk_count`,
    /// the proofs use the count the provider reports, so a provider can
    /// return another chunk of the same data for the range; only checks of
    /// the provider use that.
    async fn download_range(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        offset: u64,
        length: u64,
        chunk_count: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        let chunk_size = u64::from(DEFAULT_CHUNK_SIZE);
        let start_chunk = offset / chunk_size;
        let end_chunk = offset.saturating_add(length).div_ceil(chunk_size);
        let mut data = self
            .read_chunks(
                bucket_id,
                data_root,
                start_chunk,
                end_chunk,
                Some(chunk_size),
                chunk_count,
            )
            .await?
            .data;

        // Trim to requested range
        let start = ((offset % chunk_size) as usize).min(data.len());
        let end = start.saturating_add(length as usize).min(data.len());
        data.truncate(end);
        data.drain(..start);

        self.decrypt(data)
    }

    /// Decrypt reassembled data if encryption is enabled.
    fn decrypt(&self, data: Vec<u8>) -> ClientResult<Vec<u8>> {
        match &self.cipher {
            Some(cipher) => cipher.decrypt(&data),
            None => Ok(data),
        }
    }

    /// Download all data under `data_root`. The caller does not need to
    /// know its size or chunk size: the client rebuilds `data_root` from
    /// the chunk hashes, so a provider that leaves out or changes chunks is
    /// detected.
    pub async fn download_full(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
    ) -> ClientResult<Vec<u8>> {
        let read = self
            .read_chunks(bucket_id, data_root, 0, u64::MAX, None, None)
            .await?;
        check_root(&read, data_root)?;
        self.decrypt(read.data)
    }

    /// Download the data under `data_root` when it is at most `max` bytes,
    /// checked by rebuilding `data_root` as in [`Self::download_full`]. For
    /// longer data, returns its first `max + 1` bytes (each chunk checked
    /// against its proof), so that the caller can reject it. The data must
    /// use `DEFAULT_CHUNK_SIZE` chunks.
    pub async fn download_at_most(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        max: u64,
    ) -> ClientResult<Vec<u8>> {
        let chunk_size = u64::from(DEFAULT_CHUNK_SIZE);
        let limit = max.saturating_add(1);
        let mut read = self
            .read_chunks(
                bucket_id,
                data_root,
                0,
                limit.div_ceil(chunk_size),
                Some(chunk_size),
                None,
            )
            .await?;
        if read.hashes.len() as u64 == read.chunk_count {
            check_root(&read, data_root)?;
        }
        read.data.truncate(limit.try_into().unwrap_or(usize::MAX));
        self.decrypt(read.data)
    }

    /// Read chunks `start_chunk..end_chunk` of `data_root` in as many `/read`
    /// requests as needed, each continuing after the last returned chunk.
    /// `chunk_size` is the size of every chunk but the last; without it, the
    /// read must start at chunk 0 and takes the size from the first chunk.
    /// Fails on an incomplete or inconsistent response, another chunk size,
    /// or a chunk count other than `chunk_count`.
    async fn read_chunks(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: Option<u64>,
        chunk_count: Option<u64>,
    ) -> ClientResult<ChunkRead> {
        let mut read = ChunkRead::default();
        let mut chunk_size = chunk_size;
        // The caller's count, or the first response's.
        let mut chunk_count = chunk_count;
        let mut next = start_chunk;
        let mut end = end_chunk;
        loop {
            let size = chunk_size.unwrap_or(u64::from(DEFAULT_CHUNK_SIZE));
            let page = self
                .read_page(
                    bucket_id,
                    data_root,
                    next.saturating_mul(size),
                    end.saturating_sub(next).saturating_mul(size),
                    &mut read,
                )
                .await?;
            let count = *chunk_count.get_or_insert(page.chunk_count);
            end = end.min(count);
            // The response must continue at `next`, report the same count and
            // stay in the range.
            if page.start_chunk != next
                || page.chunk_count != count
                || page.returned > end.saturating_sub(next)
            {
                return Err(ClientError::VerificationFailed);
            }
            // Every chunk before the last one has the full chunk size.
            for (index, &len) in (next..).zip(&page.chunk_lens) {
                if index + 1 < count {
                    let full = *chunk_size.get_or_insert(len);
                    if full == 0 || len != full {
                        return Err(ClientError::VerificationFailed);
                    }
                }
            }
            next += page.returned;
            if next >= end {
                read.chunk_count = count;
                return Ok(read);
            }
            // No chunk before the end: the read is incomplete.
            if page.returned == 0 {
                return Err(ClientError::VerificationFailed);
            }
        }
    }

    /// Send one `/read` for `length` bytes from `offset`, verify each chunk's
    /// hash and proof, and append the chunk bytes and hashes to `read`.
    async fn read_page(
        &self,
        bucket_id: BucketId,
        data_root: &H256,
        offset: u64,
        length: u64,
        read: &mut ChunkRead,
    ) -> ClientResult<ReadPage> {
        let provider_url = self.base.get_provider_url()?;

        let response = self
            .base
            .http
            .get(format!("{provider_url}/read"))
            .query(&[
                ("bucket_id", bucket_id.to_string()),
                ("data_root", format!("{:#x}", data_root)),
                ("offset", offset.to_string()),
                ("length", length.to_string()),
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
        let mut chunk_lens = Vec::new();
        for (index, chunk) in read_response.chunks.into_iter().enumerate() {
            let chunk_data = BASE64
                .decode(&chunk.data)
                .map_err(|e| ClientError::Serialization(e.to_string()))?;

            let hash = hash_leaf(&chunk_data);
            let proof = chunk
                .proof
                .iter()
                .map(|h| parse_h256(h))
                .collect::<ClientResult<Vec<_>>>()?;
            let chunk_index = read_response.start_chunk.saturating_add(index as u64);
            if parse_h256(&chunk.hash)? != hash
                || !verify_merkle_proof(
                    hash,
                    chunk_index,
                    read_response.chunk_count,
                    &proof,
                    data_root,
                )
            {
                return Err(ClientError::VerificationFailed);
            }
            read.hashes.push(hash);
            chunk_lens.push(chunk_data.len() as u64);
            read.data.extend_from_slice(&chunk_data);
        }
        Ok(ReadPage {
            start_chunk: read_response.start_chunk,
            chunk_count: read_response.chunk_count,
            returned,
            chunk_lens,
        })
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
            .download_range(bucket_id, data_root, offset, chunk_size, None)
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
    /// for it yet). A provider with no agreement on the bucket answers 403
    /// `no_agreement`; this returns an error.
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

    /// Get the MMR proof of the leaf at `leaf_index`, counted from
    /// `commitment.start_seq`, in the MMR of `commitment` (`GET /mmr_proof`).
    /// The proof is not verified here: check it with
    /// [`storage_primitives::verify_mmr_proof`] against `commitment`.
    pub async fn get_mmr_proof(
        &self,
        bucket_id: BucketId,
        commitment: &storage_primitives::Commitment,
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
                ("mmr_root", format!("{:#x}", commitment.mmr_root)),
                ("start_seq", commitment.start_seq.to_string()),
                ("leaf_count", commitment.leaf_count.to_string()),
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
        self.download_range(bucket_id, data_root, offset, length, None)
            .await
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
    start_chunk: u64,
    chunk_count: u64,
    chunks: Vec<ChunkWithProof>,
}

/// Chunks read with [`StorageUserClient::read_chunks`].
#[derive(Default)]
struct ChunkRead {
    /// The chunk bytes, in order.
    data: Vec<u8>,
    /// The chunk hashes, in order.
    hashes: Vec<H256>,
    /// Chunk count of the whole data root.
    chunk_count: u64,
}

/// Fail unless `read` contains every chunk of `data_root`.
fn check_root(read: &ChunkRead, data_root: &H256) -> ClientResult<()> {
    if read.hashes.is_empty() || merkle_root(&read.hashes) != *data_root {
        return Err(ClientError::VerificationFailed);
    }
    Ok(())
}

/// Summary of one `/read` response.
struct ReadPage {
    start_chunk: u64,
    chunk_count: u64,
    returned: u64,
    /// Length of each returned chunk.
    chunk_lens: Vec<u64>,
}

#[derive(serde::Deserialize)]
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
    /// The `(mmr_root, start_seq, leaf_count)` the provider signed.
    pub fn commitment(&self) -> storage_primitives::Commitment {
        storage_primitives::Commitment {
            mmr_root: self.mmr_root,
            start_seq: self.start_seq,
            leaf_count: self.leaf_count,
        }
    }

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

    /// Chunks of the test tree: two 5-byte chunks and a shorter last one.
    const CHUNKS: [&[u8]; 3] = [b"aaaaa", b"bbbbb", b"cc"];

    fn tree_leaves() -> Vec<H256> {
        CHUNKS.iter().map(|c| hash_leaf(c)).collect()
    }

    fn tree_root() -> H256 {
        merkle_root(&tree_leaves())
    }

    /// A `/read` response that reports `start_chunk` and `chunk_count` and
    /// holds the test tree's chunks `indices`, each with its real proof.
    fn read_body(start_chunk: u64, chunk_count: u64, indices: &[usize]) -> String {
        tree_read_body(&CHUNKS, start_chunk, chunk_count, indices)
    }

    /// [`read_body`] for the tree over `tree`.
    fn tree_read_body(
        tree: &[&[u8]],
        start_chunk: u64,
        chunk_count: u64,
        indices: &[usize],
    ) -> String {
        let leaves: Vec<H256> = tree.iter().map(|c| hash_leaf(c)).collect();
        let chunks: Vec<_> = indices
            .iter()
            .map(|&i| {
                let proof: Vec<String> = storage_primitives::build_merkle_proofs(&leaves, i..i + 1)
                    .remove(0)
                    .iter()
                    .map(|h| format!("{h:#x}"))
                    .collect();
                serde_json::json!({
                    "hash": format!("{:#x}", leaves[i]),
                    "data": BASE64.encode(tree[i]),
                    "proof": proof,
                })
            })
            .collect();
        serde_json::json!({
            "start_chunk": start_chunk,
            "chunk_count": chunk_count,
            "chunks": chunks,
        })
        .to_string()
    }

    /// Serve `/read` with `body(first chunk index)`, mapping the requested
    /// offset with the test tree's 5-byte chunks, and return the provider URL.
    async fn serve_read(body: fn(u64) -> String) -> String {
        use axum::{extract::Query, routing::get, Router};
        use std::collections::HashMap;
        let app = Router::new().route(
            "/read",
            get(move |Query(q): Query<HashMap<String, String>>| async move {
                let offset: u64 = q["offset"].parse().unwrap();
                ([("content-type", "application/json")], body(offset / 5))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    async fn download_full(body: fn(u64) -> String) -> ClientResult<Vec<u8>> {
        let url = serve_read(body).await;
        let client =
            StorageUserClient::for_provider(&url, Signer::from_seed("//Alice").unwrap()).unwrap();
        client.download_full(1, &tree_root()).await
    }

    async fn assert_rejected(body: fn(u64) -> String) {
        let err = download_full(body).await.unwrap_err();
        assert!(
            matches!(err, ClientError::VerificationFailed),
            "got {err:?}"
        );
    }

    /// One chunk per response: the client continues after each one and
    /// rebuilds the root.
    #[tokio::test]
    async fn download_full_reads_one_chunk_per_response() {
        let data = download_full(|start| read_body(start, 3, &[start as usize]))
            .await
            .unwrap();
        assert_eq!(data, CHUNKS.concat());
    }

    /// A response with no chunks before the end is an incomplete read.
    #[tokio::test]
    async fn download_rejects_a_read_that_stops_before_the_chunk_count() {
        assert_rejected(|_| read_body(0, 3, &[])).await;
    }

    /// A `chunk_count` that changes between responses is inconsistent.
    #[tokio::test]
    async fn download_rejects_a_changed_chunk_count() {
        assert_rejected(|start| {
            read_body(start, if start == 0 { 3 } else { 2 }, &[start as usize])
        })
        .await;
    }

    /// A response that starts at another chunk than requested is wrong.
    #[tokio::test]
    async fn download_rejects_a_response_at_another_chunk() {
        assert_rejected(|_| read_body(1, 3, &[1])).await;
    }

    /// A chunk whose proof does not match its index is wrong.
    #[tokio::test]
    async fn download_rejects_a_chunk_at_the_wrong_index() {
        assert_rejected(|start| read_body(start, 3, &[2])).await;
    }

    /// A provider that reports fewer chunks fails the proof check.
    #[tokio::test]
    async fn download_full_rejects_a_smaller_chunk_count() {
        assert_rejected(|start| read_body(start, 2, &[start as usize])).await;
    }

    /// A provider that reports no chunks returns nothing to rebuild the root
    /// from.
    #[tokio::test]
    async fn download_full_rejects_an_empty_data_root() {
        assert_rejected(|start| read_body(start, 0, &[])).await;
    }

    async fn read_chunks(
        body: fn(u64) -> String,
        start: u64,
        end: u64,
        chunk_size: u64,
        chunk_count: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        let url = serve_read(body).await;
        let client =
            StorageUserClient::for_provider(&url, Signer::from_seed("//Alice").unwrap()).unwrap();
        let read = client
            .read_chunks(1, &tree_root(), start, end, Some(chunk_size), chunk_count)
            .await?;
        Ok(read.data)
    }

    fn assert_verification_failed(result: ClientResult<Vec<u8>>) {
        assert!(
            matches!(result, Err(ClientError::VerificationFailed)),
            "got {result:?}"
        );
    }

    /// With a known chunk count, a provider cannot pass chunk 2 as chunk 1:
    /// its proof verifies under a claimed count of 2, but the count differs.
    #[tokio::test]
    async fn read_with_a_known_count_rejects_a_chunk_at_another_position() {
        let body = |_| read_body(1, 2, &[2]);
        let without_count = read_chunks(body, 1, 2, 5, None).await;
        assert_eq!(without_count.unwrap(), CHUNKS[2], "the provider's count");
        assert_verification_failed(read_chunks(body, 1, 2, 5, Some(3)).await);
    }

    /// A first response that reaches the last chunk must still start at the
    /// requested chunk.
    #[tokio::test]
    async fn read_rejects_a_first_response_at_another_chunk() {
        assert_verification_failed(read_chunks(|_| read_body(2, 3, &[2]), 1, 3, 5, Some(3)).await);
        assert_verification_failed(
            read_chunks(|_| read_body(0, 3, &[0, 1, 2]), 1, 3, 5, Some(3)).await,
        );
        let data = read_chunks(|start| read_body(start, 3, &[1, 2]), 1, 3, 5, Some(3)).await;
        assert_eq!(data.unwrap(), [CHUNKS[1], CHUNKS[2]].concat());
    }

    /// A range read rejects chunks of another size than the one it maps
    /// bytes with.
    #[tokio::test]
    async fn read_rejects_another_chunk_size() {
        assert_verification_failed(
            read_chunks(
                |start| read_body(start, 3, &[start as usize]),
                0,
                3,
                4,
                Some(3),
            )
            .await,
        );
    }

    /// A short chunk in the middle of a response is rejected, not only a
    /// short first chunk.
    #[tokio::test]
    async fn read_rejects_a_short_chunk_after_the_first() {
        const MIXED: [&[u8]; 3] = [b"aaaaa", b"bb", b"ccccc"];
        let url = serve_read(|_| tree_read_body(&MIXED, 0, 3, &[0, 1, 2])).await;
        let client =
            StorageUserClient::for_provider(&url, Signer::from_seed("//Alice").unwrap()).unwrap();
        let leaves: Vec<H256> = MIXED.iter().map(|c| hash_leaf(c)).collect();
        let read = client
            .read_chunks(1, &merkle_root(&leaves), 0, 3, Some(5), Some(3))
            .await
            .map(|read| read.data);
        assert_verification_failed(read);
    }

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
