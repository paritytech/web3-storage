// SPDX-License-Identifier: Apache-2.0

//! S3-Compatible Client SDK for Web3 Storage
//!
//! This crate provides a high-level S3-compatible API on top of the Layer 0 storage.

mod substrate;

pub use substrate::SubstrateClient;

use s3_primitives::{
    validate_bucket_name, validate_object_key, ListObjectsParams, ListObjectsResponse, S3BucketId,
};
use std::collections::HashMap;
use storage_subxt::subxt::utils::H256;
use thiserror::Error;
use tracing::{debug, info};

/// S3 client error types.
#[derive(Error, Debug)]
pub enum S3ClientError {
    #[error("Bucket not found: {0}")]
    BucketNotFound(String),

    #[error("Object not found: {bucket}/{key}")]
    ObjectNotFound { bucket: String, key: String },

    #[error("Bucket already exists: {0}")]
    BucketAlreadyExists(String),

    #[error("Invalid bucket name: {0}")]
    InvalidBucketName(String),

    #[error("Invalid object key: {0}")]
    InvalidObjectKey(String),

    #[error("Access denied")]
    AccessDenied,

    #[error("Chain error: {0}")]
    ChainError(String),

    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Result type for S3 client operations.
pub type Result<T> = std::result::Result<T, S3ClientError>;

/// Options for put_object operation.
#[derive(Default, Clone, Debug)]
pub struct PutObjectOptions {
    /// Content type (MIME type).
    pub content_type: Option<String>,
    /// User-defined metadata.
    pub metadata: HashMap<String, String>,
}

/// Response from put_object operation.
#[derive(Clone, Debug)]
pub struct PutObjectResponse {
    /// ETag of the uploaded object.
    pub etag: String,
    /// CID of the uploaded object.
    pub cid: H256,
    /// Size of the uploaded object.
    pub size: u64,
}

/// Response from get_object operation.
#[derive(Clone, Debug)]
pub struct GetObjectResponse {
    /// Object data.
    pub data: Vec<u8>,
    /// Content type.
    pub content_type: String,
    /// ETag.
    pub etag: String,
    /// Size.
    pub size: u64,
    /// Last modified timestamp.
    pub last_modified: u64,
    /// User metadata.
    pub metadata: HashMap<String, String>,
}

/// Response from head_object operation.
#[derive(Clone, Debug)]
pub struct HeadObjectResponse {
    /// Content type.
    pub content_type: String,
    /// ETag.
    pub etag: String,
    /// Size.
    pub size: u64,
    /// Last modified timestamp.
    pub last_modified: u64,
    /// CID.
    pub cid: H256,
    /// User metadata.
    pub metadata: HashMap<String, String>,
}

/// Bucket information.
#[derive(Clone, Debug)]
pub struct BucketInfo {
    /// S3 bucket ID.
    pub s3_bucket_id: S3BucketId,
    /// Bucket name.
    pub name: String,
    /// Layer 0 bucket ID.
    pub layer0_bucket_id: u64,
    /// Object count.
    pub object_count: u64,
    /// Total size.
    pub total_size: u64,
    /// Creation timestamp (block number).
    pub created_at: u32,
}

/// S3 client for interacting with web3-storage using S3-compatible semantics.
pub struct S3Client {
    /// Layer 0 storage client for blob operations.
    storage_client: storage_client::StorageUserClient,
    /// Substrate client for chain operations.
    substrate_client: SubstrateClient,
}

impl S3Client {
    /// Create a new S3 client.
    pub async fn new(chain_url: &str, provider_url: &str, seed_phrase: &str) -> Result<Self> {
        info!(
            "Creating S3 client with chain={}, provider={}",
            chain_url, provider_url
        );

        let config = storage_client::ClientConfig {
            chain_ws_url: chain_url.to_string(),
            provider_urls: vec![provider_url.to_string()],
            ..Default::default()
        };
        let storage_client = storage_client::StorageUserClient::new(config)
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        let substrate_client = SubstrateClient::new(chain_url, seed_phrase)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        Ok(Self {
            storage_client,
            substrate_client,
        })
    }

    /// Create a new S3 bucket.
    ///
    /// `terms` + `sig` are the provider-signed agreement bundle returned by
    /// [`storage_client::ProviderClient::negotiate_terms`]. The Layer 0 bucket
    /// + primary agreement open atomically alongside the S3 bucket.
    pub async fn create_bucket(
        &self,
        name: &str,
        provider: storage_subxt::subxt::utils::AccountId32,
        terms: storage_client::AgreementTermsOf,
        sig: storage_subxt::api::runtime_types::sp_runtime::MultiSignature,
    ) -> Result<BucketInfo> {
        info!("Creating bucket: {}", name);

        if !validate_bucket_name(name.as_bytes()) {
            return Err(S3ClientError::InvalidBucketName(name.to_string()));
        }

        // Create S3 bucket (Layer 0 bucket is created internally by the pallet)
        // The pallet validates name uniqueness, so no need to pre-check.
        let s3_bucket_id = self
            .substrate_client
            .create_s3_bucket(name, provider, &terms, sig)
            .await
            .map_err(S3ClientError::ChainError)?;

        // Fetch the created bucket info to get the layer0_bucket_id
        let bucket_info = self
            .substrate_client
            .get_bucket_info(s3_bucket_id)
            .await
            .map_err(S3ClientError::ChainError)?
            .ok_or_else(|| {
                S3ClientError::InternalError("Bucket created but not found".to_string())
            })?;

        info!(
            "S3 bucket created: {} (s3_id={}, layer0_id={})",
            name, s3_bucket_id, bucket_info.layer0_bucket_id
        );

        Ok(bucket_info)
    }

    /// Delete an S3 bucket.
    pub async fn delete_bucket(&self, name: &str) -> Result<()> {
        info!("Deleting bucket: {}", name);

        let bucket_id = self
            .substrate_client
            .get_bucket_id_by_name(name)
            .await?
            .ok_or_else(|| S3ClientError::BucketNotFound(name.to_string()))?;

        self.substrate_client
            .delete_s3_bucket(bucket_id)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        info!("Bucket deleted: {}", name);
        Ok(())
    }

    /// Get bucket information.
    pub async fn head_bucket(&self, name: &str) -> Result<BucketInfo> {
        let bucket_id = self
            .substrate_client
            .get_bucket_id_by_name(name)
            .await?
            .ok_or_else(|| S3ClientError::BucketNotFound(name.to_string()))?;

        self.substrate_client
            .get_bucket_info(bucket_id)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?
            .ok_or_else(|| S3ClientError::BucketNotFound(name.to_string()))
    }

    /// List all buckets owned by the user.
    pub async fn list_buckets(&self) -> Result<Vec<BucketInfo>> {
        self.substrate_client
            .list_user_buckets()
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))
    }

    /// Upload an object to a bucket.
    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        options: PutObjectOptions,
    ) -> Result<PutObjectResponse> {
        info!(
            "Uploading object: {}/{} ({} bytes)",
            bucket,
            key,
            data.len()
        );

        if !validate_object_key(key.as_bytes()) {
            return Err(S3ClientError::InvalidObjectKey(key.to_string()));
        }

        let bucket_info = self.head_bucket(bucket).await?;

        debug!("Uploading to provider");

        // Upload to provider — the returned data_root is the Merkle tree root
        // used to retrieve data from the provider's storage layer.
        let data_root = self
            .storage_client
            .upload(bucket_info.layer0_bucket_id, data, Default::default())
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        // Use data_root as the CID so download can find the data
        let cid = data_root;

        let content_type = options
            .content_type
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let metadata_vec: Vec<(Vec<u8>, Vec<u8>)> = options
            .metadata
            .into_iter()
            .map(|(k, v)| (k.into_bytes(), v.into_bytes()))
            .collect();

        debug!("Storing object metadata on chain");
        self.substrate_client
            .put_object_metadata(
                bucket_info.s3_bucket_id,
                key,
                cid,
                data.len() as u64,
                &content_type,
                metadata_vec,
            )
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        let etag = hex::encode(cid.as_bytes());
        info!("Object uploaded: {}/{} (etag={})", bucket, key, etag);

        Ok(PutObjectResponse {
            etag,
            cid,
            size: data.len() as u64,
        })
    }

    /// Download an object from a bucket.
    pub async fn get_object(&self, bucket: &str, key: &str) -> Result<GetObjectResponse> {
        info!("Downloading object: {}/{}", bucket, key);

        let bucket_info = self.head_bucket(bucket).await?;

        let metadata = self
            .substrate_client
            .get_object_metadata(bucket_info.s3_bucket_id, key)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?
            .ok_or_else(|| S3ClientError::ObjectNotFound {
                bucket: bucket.to_string(),
                key: key.to_string(),
            })?;

        debug!("Downloading from provider, CID: {:?}", metadata.cid);
        let data = self
            .storage_client
            .download_full(&metadata.cid, metadata.size)
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        info!(
            "Object downloaded: {}/{} ({} bytes)",
            bucket,
            key,
            data.len()
        );

        Ok(GetObjectResponse {
            data,
            content_type: String::from_utf8_lossy(&metadata.content_type).to_string(),
            etag: String::from_utf8_lossy(&metadata.etag).to_string(),
            size: metadata.size,
            last_modified: metadata.last_modified,
            metadata: metadata
                .user_metadata
                .into_iter()
                .map(|e| {
                    (
                        String::from_utf8_lossy(&e.key).to_string(),
                        String::from_utf8_lossy(&e.value).to_string(),
                    )
                })
                .collect(),
        })
    }

    /// Delete an object from a bucket.
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        info!("Deleting object: {}/{}", bucket, key);

        let bucket_info = self.head_bucket(bucket).await?;

        self.substrate_client
            .delete_object_metadata(bucket_info.s3_bucket_id, key)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        info!("Object deleted: {}/{}", bucket, key);
        Ok(())
    }

    /// Copy an object from one location to another.
    pub async fn copy_object(
        &self,
        src_bucket: &str,
        src_key: &str,
        dst_bucket: &str,
        dst_key: &str,
    ) -> Result<PutObjectResponse> {
        info!(
            "Copying object: {}/{} -> {}/{}",
            src_bucket, src_key, dst_bucket, dst_key
        );

        let (src_bucket_info, dst_bucket_info) =
            tokio::try_join!(self.head_bucket(src_bucket), self.head_bucket(dst_bucket))?;

        self.substrate_client
            .copy_object_metadata(
                src_bucket_info.s3_bucket_id,
                src_key,
                dst_bucket_info.s3_bucket_id,
                dst_key,
            )
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        // Read the copied object's metadata from the destination
        let dst_metadata = self
            .substrate_client
            .get_object_metadata(dst_bucket_info.s3_bucket_id, dst_key)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?
            .ok_or_else(|| S3ClientError::ObjectNotFound {
                bucket: dst_bucket.to_string(),
                key: dst_key.to_string(),
            })?;

        info!(
            "Object copied: {}/{} -> {}/{}",
            src_bucket, src_key, dst_bucket, dst_key
        );

        Ok(PutObjectResponse {
            etag: String::from_utf8_lossy(&dst_metadata.etag).to_string(),
            cid: dst_metadata.cid,
            size: dst_metadata.size,
        })
    }

    /// List objects in a bucket.
    pub async fn list_objects_v2(
        &self,
        bucket: &str,
        params: ListObjectsParams,
    ) -> Result<ListObjectsResponse> {
        debug!("Listing objects in bucket: {}", bucket);

        let bucket_info = self.head_bucket(bucket).await?;

        self.substrate_client
            .list_objects(bucket_info.s3_bucket_id, params)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_object_options_default() {
        let options = PutObjectOptions::default();
        assert!(options.content_type.is_none());
        assert!(options.metadata.is_empty());
    }
}
