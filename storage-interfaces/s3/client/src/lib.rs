//! S3-Compatible Client SDK for Web3 Storage
//!
//! This crate provides a high-level S3-compatible API on top of the Layer 0 storage.
//! Object operations go directly to the provider node's HTTP API.
//! Only bucket operations touch the chain.

mod substrate;

pub use substrate::SubstrateClient;

use s3_primitives::{validate_bucket_name, S3BucketId};
use sp_core::H256;
use std::collections::HashMap;
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
    /// CID (data_root) of the uploaded object.
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
    /// Data root.
    pub data_root: String,
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
    /// Creation timestamp (block number).
    pub created_at: u32,
}

/// S3 client for interacting with web3-storage using S3-compatible semantics.
pub struct S3Client {
    /// Provider HTTP base URL.
    provider_url: String,
    /// HTTP client for provider requests.
    http_client: reqwest::Client,
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

        let substrate_client = SubstrateClient::new(chain_url, seed_phrase)
            .await
            .map_err(|e| S3ClientError::ChainError(e.to_string()))?;

        Ok(Self {
            provider_url: provider_url.trim_end_matches('/').to_string(),
            http_client: reqwest::Client::new(),
            substrate_client,
        })
    }

    /// Create a new S3 bucket.
    pub async fn create_bucket(&self, name: &str) -> Result<BucketInfo> {
        self.create_bucket_with_options(name, 1).await
    }

    /// Create a new S3 bucket with custom options.
    pub async fn create_bucket_with_options(
        &self,
        name: &str,
        min_providers: u32,
    ) -> Result<BucketInfo> {
        info!(
            "Creating bucket: {} (min_providers={})",
            name, min_providers
        );

        if !validate_bucket_name(name.as_bytes()) {
            return Err(S3ClientError::InvalidBucketName(name.to_string()));
        }

        if self
            .substrate_client
            .get_bucket_id_by_name(name)
            .await?
            .is_some()
        {
            return Err(S3ClientError::BucketAlreadyExists(name.to_string()));
        }

        let s3_bucket_id = self
            .substrate_client
            .create_s3_bucket(name, min_providers)
            .await
            .map_err(S3ClientError::ChainError)?;

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

    /// Upload an object to a bucket via the provider's S3 HTTP API.
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

        let bucket_info = self.head_bucket(bucket).await?;
        let url = format!(
            "{}/s3/{}/object?key={}",
            self.provider_url,
            bucket_info.layer0_bucket_id,
            urlencoding::encode(key)
        );

        let content_type = options
            .content_type
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let mut request = self
            .http_client
            .put(&url)
            .header("content-type", &content_type)
            .body(data.to_vec());

        for (k, v) in &options.metadata {
            request = request.header(format!("x-amz-meta-{k}"), v);
        }

        let response = request
            .send()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            return Err(S3ClientError::ProviderError(format!(
                "PUT failed: {status} {body}"
            )));
        }

        let put_resp: serde_json::Value = response
            .json()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        let etag = put_resp["etag"].as_str().unwrap_or_default().to_string();
        let data_root_str = put_resp["data_root"].as_str().unwrap_or_default();
        let cid = parse_h256(data_root_str);
        let size = put_resp["size"].as_u64().unwrap_or(data.len() as u64);

        info!("Object uploaded: {}/{} (etag={})", bucket, key, etag);

        Ok(PutObjectResponse { etag, cid, size })
    }

    /// Download an object from a bucket via the provider's S3 HTTP API.
    pub async fn get_object(&self, bucket: &str, key: &str) -> Result<GetObjectResponse> {
        info!("Downloading object: {}/{}", bucket, key);

        let bucket_info = self.head_bucket(bucket).await?;
        let url = format!(
            "{}/s3/{}/object?key={}",
            self.provider_url,
            bucket_info.layer0_bucket_id,
            urlencoding::encode(key)
        );

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        if response.status().as_u16() == 404 {
            return Err(S3ClientError::ObjectNotFound {
                bucket: bucket.to_string(),
                key: key.to_string(),
            });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            return Err(S3ClientError::ProviderError(format!(
                "GET failed: {status} {body}"
            )));
        }

        let headers = response.headers().clone();
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let etag = headers
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let size = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let last_modified = headers
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        let mut metadata = HashMap::new();
        for (name, value) in headers.iter() {
            let name_str = name.as_str().to_lowercase();
            if let Some(meta_key) = name_str.strip_prefix("x-amz-meta-") {
                if let Ok(v) = value.to_str() {
                    metadata.insert(meta_key.to_string(), v.to_string());
                }
            }
        }

        let data = response
            .bytes()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?
            .to_vec();

        info!(
            "Object downloaded: {}/{} ({} bytes)",
            bucket,
            key,
            data.len()
        );

        Ok(GetObjectResponse {
            data,
            content_type,
            etag,
            size,
            last_modified,
            metadata,
        })
    }

    /// Delete an object from a bucket via the provider's S3 HTTP API.
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        info!("Deleting object: {}/{}", bucket, key);

        let bucket_info = self.head_bucket(bucket).await?;
        let url = format!(
            "{}/s3/{}/object?key={}",
            self.provider_url,
            bucket_info.layer0_bucket_id,
            urlencoding::encode(key)
        );

        let response = self
            .http_client
            .delete(&url)
            .send()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            return Err(S3ClientError::ProviderError(format!(
                "DELETE failed: {status} {body}"
            )));
        }

        info!("Object deleted: {}/{}", bucket, key);
        Ok(())
    }

    /// Copy an object by downloading from source and uploading to destination.
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

        // Download source
        let src = self.get_object(src_bucket, src_key).await?;

        // Upload to destination
        let result = self
            .put_object(
                dst_bucket,
                dst_key,
                &src.data,
                PutObjectOptions {
                    content_type: Some(src.content_type),
                    metadata: src.metadata,
                },
            )
            .await?;

        info!(
            "Object copied: {}/{} -> {}/{}",
            src_bucket, src_key, dst_bucket, dst_key
        );

        Ok(result)
    }

    /// Get object metadata without downloading the body.
    pub async fn head_object(&self, bucket: &str, key: &str) -> Result<HeadObjectResponse> {
        debug!("HEAD object: {}/{}", bucket, key);

        let bucket_info = self.head_bucket(bucket).await?;
        let url = format!(
            "{}/s3/{}/object?key={}",
            self.provider_url,
            bucket_info.layer0_bucket_id,
            urlencoding::encode(key)
        );

        let response = self
            .http_client
            .head(&url)
            .send()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        if response.status().as_u16() == 404 {
            return Err(S3ClientError::ObjectNotFound {
                bucket: bucket.to_string(),
                key: key.to_string(),
            });
        }

        if !response.status().is_success() {
            let status = response.status();
            return Err(S3ClientError::ProviderError(format!(
                "HEAD failed: {status}"
            )));
        }

        let headers = response.headers();
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let etag = headers
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let size = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let last_modified = headers
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let data_root = headers
            .get("x-amz-data-root")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        let mut metadata = HashMap::new();
        for (name, value) in headers.iter() {
            let name_str = name.as_str().to_lowercase();
            if let Some(meta_key) = name_str.strip_prefix("x-amz-meta-") {
                if let Ok(v) = value.to_str() {
                    metadata.insert(meta_key.to_string(), v.to_string());
                }
            }
        }

        Ok(HeadObjectResponse {
            content_type,
            etag,
            size,
            last_modified,
            data_root,
            metadata,
        })
    }

    /// List objects in a bucket via the provider's S3 HTTP API.
    pub async fn list_objects_v2(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        delimiter: Option<&str>,
        max_keys: Option<u32>,
    ) -> Result<serde_json::Value> {
        debug!("Listing objects in bucket: {}", bucket);

        let bucket_info = self.head_bucket(bucket).await?;
        let mut url = format!(
            "{}/s3/{}/objects",
            self.provider_url, bucket_info.layer0_bucket_id
        );

        let mut params = Vec::new();
        if let Some(p) = prefix {
            params.push(format!("prefix={}", urlencoding::encode(p)));
        }
        if let Some(d) = delimiter {
            params.push(format!("delimiter={}", urlencoding::encode(d)));
        }
        if let Some(mk) = max_keys {
            params.push(format!("max_keys={mk}"));
        }
        if !params.is_empty() {
            url = format!("{}?{}", url, params.join("&"));
        }

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            return Err(S3ClientError::ProviderError(format!(
                "LIST failed: {status} {body}"
            )));
        }

        response
            .json()
            .await
            .map_err(|e| S3ClientError::ProviderError(e.to_string()))
    }
}

/// Parse a hex string (with or without 0x prefix) into an H256.
fn parse_h256(hex_str: &str) -> H256 {
    let clean = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    if clean.len() == 64 {
        if let Ok(bytes) = hex::decode(clean) {
            return H256::from_slice(&bytes);
        }
    }
    H256::zero()
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

    #[test]
    fn test_parse_h256() {
        let hex_str = "0x4242424242424242424242424242424242424242424242424242424242424242";
        let h = parse_h256(hex_str);
        assert_eq!(h, H256::repeat_byte(0x42));

        // Without prefix
        let hex_str2 = "4242424242424242424242424242424242424242424242424242424242424242";
        assert_eq!(parse_h256(hex_str2), H256::repeat_byte(0x42));

        // Invalid returns zero
        assert_eq!(parse_h256("invalid"), H256::zero());
    }
}
