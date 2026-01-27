//! # Storage Client Library
//!
//! Client library for interacting with the scalable Web3 storage system.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use storage_client::{StorageClient, ChunkingStrategy};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = StorageClient::new("http://localhost:3000");
//!
//!     // Upload data
//!     let data = b"Hello, World!";
//!     let data_root = client.upload(1, data, ChunkingStrategy::Fixed(256 * 1024)).await?;
//!
//!     // Commit to MMR
//!     let commit = client.commit(1, vec![data_root]).await?;
//!
//!     // Read back
//!     let read_data = client.read(&data_root, 0, data.len() as u64).await?;
//!     assert_eq!(read_data, data);
//!
//!     Ok(())
//! }
//! ```

pub mod verification;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::collections::HashMap;
use storage_primitives::{blake2_256, BucketId};
use thiserror::Error;

/// Client errors.
#[derive(Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Data verification failed")]
    VerificationFailed,
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

/// Storage client for interacting with provider nodes.
pub struct StorageClient {
    http: Client,
    base_url: String,
}

impl StorageClient {
    /// Create a new storage client.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Upload data to the provider and return the data root.
    pub async fn upload(
        &self,
        bucket_id: BucketId,
        data: &[u8],
        strategy: ChunkingStrategy,
    ) -> Result<H256, Error> {
        // Chunk the data
        let chunks = self.chunk_data(data, strategy);

        // Upload chunks (leaves)
        let chunk_hashes: Vec<H256> = chunks
            .iter()
            .map(|chunk| blake2_256(chunk))
            .collect();

        for (chunk, hash) in chunks.iter().zip(chunk_hashes.iter()) {
            self.upload_node(bucket_id, *hash, chunk.clone(), None).await?;
        }

        // Build Merkle tree bottom-up
        let data_root = self.build_merkle_tree(bucket_id, &chunk_hashes).await?;

        Ok(data_root)
    }

    /// Commit data roots to the bucket's MMR.
    pub async fn commit(
        &self,
        bucket_id: BucketId,
        data_roots: Vec<H256>,
    ) -> Result<CommitResponse, Error> {
        let request = CommitRequest {
            bucket_id,
            data_roots: data_roots
                .iter()
                .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                .collect(),
        };

        let response = self
            .http
            .post(format!("{}/commit", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
            return Err(Error::Api(error.error));
        }

        response.json().await.map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Read data from a data root.
    pub async fn read(&self, data_root: &H256, offset: u64, length: u64) -> Result<Vec<u8>, Error> {
        let response = self
            .http
            .get(format!("{}/read", self.base_url))
            .query(&[
                ("data_root", format!("0x{}", hex_encode(data_root.as_bytes()))),
                ("offset", offset.to_string()),
                ("length", length.to_string()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
            return Err(Error::Api(error.error));
        }

        let read_response: ReadResponse = response.json().await?;

        let mut data = Vec::new();
        for chunk in read_response.chunks {
            let chunk_data = BASE64
                .decode(&chunk.data)
                .map_err(|e| Error::Serialization(e.to_string()))?;

            // Verify chunk hash
            let expected_hash = hex_decode(&chunk.hash)
                .map_err(|e| Error::Serialization(e.to_string()))?;
            let actual_hash = blake2_256(&chunk_data);
            if actual_hash.as_bytes() != expected_hash.as_slice() {
                return Err(Error::VerificationFailed);
            }

            data.extend_from_slice(&chunk_data);
        }

        // Trim to requested range
        let start = (offset % (256 * 1024)) as usize;
        let end = start + length as usize;
        if end <= data.len() {
            Ok(data[start..end].to_vec())
        } else {
            Ok(data[start..].to_vec())
        }
    }

    /// Get the current commitment for a bucket.
    pub async fn get_commitment(&self, bucket_id: BucketId) -> Result<CommitmentResponse, Error> {
        let response = self
            .http
            .get(format!("{}/commitment", self.base_url))
            .query(&[("bucket_id", bucket_id.to_string())])
            .send()
            .await?;

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
            return Err(Error::Api(error.error));
        }

        response.json().await.map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Check which nodes exist on the provider.
    pub async fn check_exists(
        &self,
        bucket_id: BucketId,
        hashes: Vec<H256>,
    ) -> Result<ExistsResponse, Error> {
        let request = ExistsRequest {
            bucket_id,
            hashes: hashes
                .iter()
                .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                .collect(),
        };

        let response = self
            .http
            .post(format!("{}/exists", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
            return Err(Error::Api(error.error));
        }

        response.json().await.map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Health check.
    pub async fn health(&self) -> Result<HealthResponse, Error> {
        let response = self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await?;

        response.json().await.map_err(|e| Error::Serialization(e.to_string()))
    }

    // Private helper methods

    fn chunk_data(&self, data: &[u8], strategy: ChunkingStrategy) -> Vec<Vec<u8>> {
        match strategy {
            ChunkingStrategy::Fixed(chunk_size) => {
                data.chunks(chunk_size).map(|c| c.to_vec()).collect()
            }
            ChunkingStrategy::ContentDefined => {
                // Not implemented - fall back to fixed
                self.chunk_data(data, ChunkingStrategy::Fixed(256 * 1024))
            }
        }
    }

    async fn upload_node(
        &self,
        bucket_id: BucketId,
        hash: H256,
        data: Vec<u8>,
        children: Option<Vec<H256>>,
    ) -> Result<(), Error> {
        let request = UploadNodeRequest {
            bucket_id,
            hash: format!("0x{}", hex_encode(hash.as_bytes())),
            data: BASE64.encode(&data),
            children: children.map(|c| {
                c.iter()
                    .map(|h| format!("0x{}", hex_encode(h.as_bytes())))
                    .collect()
            }),
        };

        let response = self
            .http
            .put(format!("{}/node", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
            return Err(Error::Api(error.error));
        }

        Ok(())
    }

    async fn build_merkle_tree(
        &self,
        bucket_id: BucketId,
        leaves: &[H256],
    ) -> Result<H256, Error> {
        if leaves.is_empty() {
            return Ok(H256::zero());
        }
        if leaves.len() == 1 {
            return Ok(leaves[0]);
        }

        let mut current_level = leaves.to_vec();

        while current_level.len() > 1 {
            let mut next_level = Vec::new();

            for chunk in current_level.chunks(2) {
                if chunk.len() == 2 {
                    let parent = storage_primitives::hash_children(chunk[0], chunk[1]);

                    // Create internal node data (concatenated child hashes)
                    let mut node_data = Vec::new();
                    node_data.extend_from_slice(chunk[0].as_bytes());
                    node_data.extend_from_slice(chunk[1].as_bytes());

                    // Upload internal node
                    self.upload_node(
                        bucket_id,
                        parent,
                        node_data,
                        Some(vec![chunk[0], chunk[1]]),
                    )
                    .await?;

                    next_level.push(parent);
                } else {
                    // Odd node - promote to next level
                    next_level.push(chunk[0]);
                }
            }

            current_level = next_level;
        }

        Ok(current_level[0])
    }
}

// API types (matching provider-node)

#[derive(Serialize)]
struct UploadNodeRequest {
    bucket_id: BucketId,
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

#[derive(Serialize)]
struct ExistsRequest {
    bucket_id: BucketId,
    hashes: Vec<String>,
}

#[derive(Deserialize)]
pub struct ExistsResponse {
    pub exists: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Serialize)]
struct CommitRequest {
    bucket_id: BucketId,
    data_roots: Vec<String>,
}

#[derive(Deserialize)]
pub struct CommitResponse {
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_indices: Vec<u64>,
    pub provider_signature: String,
}

#[derive(Deserialize)]
struct ReadResponse {
    chunks: Vec<ChunkData>,
}

#[derive(Deserialize)]
struct ChunkData {
    hash: String,
    data: String,
    proof: Vec<String>,
}

#[derive(Deserialize)]
pub struct CommitmentResponse {
    pub bucket_id: BucketId,
    pub mmr_root: String,
    pub start_seq: u64,
    pub leaf_count: u64,
    pub provider_signature: String,
}

#[derive(Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Deserialize)]
struct ApiError {
    error: String,
}

// Hex utilities

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() % 2 != 0 {
        return Err("invalid hex length");
    }

    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunking() {
        let client = StorageClient::new("http://localhost:3000");

        let data = vec![0u8; 1024 * 1024]; // 1 MiB
        let chunks = client.chunk_data(&data, ChunkingStrategy::Fixed(256 * 1024));

        assert_eq!(chunks.len(), 4);
        for chunk in &chunks {
            assert_eq!(chunk.len(), 256 * 1024);
        }
    }
}
