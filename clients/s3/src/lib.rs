// SPDX-License-Identifier: Apache-2.0

//! S3-compatible client for Web3 Storage.
//!
//! An S3 bucket is a Layer 0 bucket of `pallet-storage-provider`, identified
//! by its bucket id. The chain stores no bucket name and no object metadata.
//! Object operations go to the provider's `/s3/{bucket_id}/...` HTTP routes;
//! the provider's S3 index maps each key to its content, content type and
//! user metadata.
//!
//! Downloads by key are unverified: nothing on chain commits to the
//! provider's key-to-content index, so the client cannot check that the
//! returned bytes belong to the requested key (#410).
//!
//! The client has no bucket deletion: Layer 0 has no bucket deletion.

mod substrate;

pub use storage_client::Signer;
pub use storage_primitives::{BucketId, Role, Visibility};
pub use substrate::SubstrateClient;

use reqwest::{Response, StatusCode};
use serde::Deserialize;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::collections::HashMap;
use thiserror::Error;
use tracing::{debug, info};

/// Prefix of the HTTP headers that carry user metadata.
const USER_METADATA_HEADER_PREFIX: &str = "x-amz-meta-";

/// Maximum object key length in bytes.
const MAX_OBJECT_KEY_LEN: usize = 1024;

/// S3 client error types.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum S3ClientError {
    /// No Layer 0 bucket with this id exists.
    #[error("Bucket not found: {0}")]
    BucketNotFound(BucketId),

    /// The provider's index has no object with this key.
    #[error("Object not found: {bucket_id}/{key}")]
    ObjectNotFound { bucket_id: BucketId, key: String },

    /// The key is empty or longer than 1024 bytes.
    #[error("Invalid object key: {0}")]
    InvalidObjectKey(String),

    /// The provider rejected the request: the signer is not a bucket member
    /// with the required role.
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

/// Options for [`S3Client::put_object`].
#[derive(Default, Clone, Debug)]
pub struct PutObjectOptions {
    /// Content type (MIME type). Defaults to `application/octet-stream`.
    pub content_type: Option<String>,
    /// User-defined metadata, sent as `x-amz-meta-*` headers. Header names
    /// are case-insensitive, so keys come back lowercase. A key or value that
    /// is not valid in an HTTP header fails the request with
    /// [`S3ClientError::HttpError`].
    pub metadata: HashMap<String, String>,
}

/// Response from [`S3Client::put_object`].
#[derive(Clone, Debug)]
pub struct PutObjectResponse {
    /// ETag of the uploaded object.
    pub etag: String,
    /// Merkle root of the uploaded data.
    pub cid: H256,
    /// Size of the uploaded object in bytes.
    pub size: u64,
}

/// Response from [`S3Client::get_object`].
#[derive(Clone, Debug)]
pub struct GetObjectResponse {
    /// Object data. Unverified: see the crate docs.
    pub data: Vec<u8>,
    /// Content type.
    pub content_type: String,
    /// ETag.
    pub etag: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modified time, in seconds since the Unix epoch.
    pub last_modified: u64,
    /// User metadata.
    pub metadata: HashMap<String, String>,
}

/// Response from [`S3Client::head_object`].
#[derive(Clone, Debug)]
pub struct HeadObjectResponse {
    /// Content type.
    pub content_type: String,
    /// ETag.
    pub etag: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modified time, in seconds since the Unix epoch.
    pub last_modified: u64,
    /// Merkle root of the object data, as reported by the provider.
    pub cid: H256,
    /// User metadata.
    pub metadata: HashMap<String, String>,
}

/// A member of a Layer 0 bucket.
#[derive(Clone, Debug)]
pub struct BucketMember {
    /// Member account.
    pub account: AccountId32,
    /// Member role.
    pub role: Role,
}

/// A Layer 0 bucket, read from `StorageProvider::Buckets`.
#[derive(Clone, Debug)]
pub struct BucketInfo {
    /// Layer 0 bucket id.
    pub bucket_id: BucketId,
    /// Bucket members.
    pub members: Vec<BucketMember>,
    /// Primary providers of the bucket.
    pub primary_providers: Vec<AccountId32>,
    /// Read visibility.
    pub visibility: Visibility,
    /// `true` if the bucket is frozen.
    pub frozen: bool,
}

/// Parameters for [`S3Client::list_objects_v2`].
#[derive(Clone, Debug, Default)]
pub struct ListObjectsParams {
    /// Return only keys that start with this prefix.
    pub prefix: Option<String>,
    /// Group keys that contain this delimiter after the prefix into
    /// `common_prefixes`.
    pub delimiter: Option<String>,
    /// Return only keys after this key.
    pub start_after: Option<String>,
    /// `next_continuation_token` from the previous page.
    pub continuation_token: Option<String>,
    /// Maximum number of keys to return. The provider default is 1000.
    pub max_keys: Option<u32>,
}

/// One object in a [`ListObjectsResponse`].
#[derive(Clone, Debug, Deserialize)]
pub struct ObjectSummary {
    /// Object key.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modified time, in seconds since the Unix epoch.
    pub last_modified: u64,
    /// ETag.
    pub etag: String,
}

/// Response from [`S3Client::list_objects_v2`], as returned by the provider.
#[derive(Clone, Debug, Deserialize)]
pub struct ListObjectsResponse {
    /// Matching objects.
    pub contents: Vec<ObjectSummary>,
    /// Key prefixes grouped by the delimiter.
    pub common_prefixes: Vec<String>,
    /// `true` if more keys match than this page returns.
    pub is_truncated: bool,
    /// Token for the next page.
    pub next_continuation_token: Option<String>,
    /// Number of keys returned.
    pub key_count: u32,
}

/// Body of the provider's `PUT /s3/{bucket_id}/object` response.
#[derive(Deserialize)]
struct PutObjectWire {
    etag: String,
    data_root: String,
    size: u64,
}

/// S3 client for interacting with web3-storage using S3-compatible semantics.
pub struct S3Client {
    http: reqwest::Client,
    provider_url: String,
    signer: Signer,
    substrate_client: SubstrateClient,
}

impl S3Client {
    /// Create a new S3 client.
    ///
    /// `signer` signs extrinsics and authenticates every provider request.
    /// All object operations go to `provider_url`.
    pub async fn new(chain_url: &str, provider_url: &str, signer: Signer) -> Result<Self> {
        info!(
            "Creating S3 client with chain={}, provider={}",
            chain_url, provider_url
        );

        let substrate_client = SubstrateClient::new(chain_url, signer.clone()).await?;

        Ok(Self {
            http: reqwest::Client::new(),
            provider_url: provider_url.trim_end_matches('/').to_string(),
            signer,
            substrate_client,
        })
    }

    /// Create an S3 bucket: a Layer 0 bucket with one primary agreement.
    ///
    /// Submits `StorageProvider::create_bucket_with_primary`. `terms` and
    /// `sig` are the provider-signed agreement returned by
    /// [`storage_client::ProviderClient::negotiate_terms`]. The signer
    /// becomes the bucket admin. Returns the new bucket id.
    pub async fn create_bucket(
        &self,
        provider: AccountId32,
        terms: storage_client::AgreementTermsOf,
        sig: sp_runtime::MultiSignature,
        visibility: Visibility,
    ) -> Result<BucketId> {
        let bucket_id = self
            .substrate_client
            .create_bucket_with_primary(provider, &terms, &sig, visibility)
            .await?;
        info!("S3 bucket created: {}", bucket_id);
        Ok(bucket_id)
    }

    /// Read a bucket from the chain. Fails with
    /// [`S3ClientError::BucketNotFound`] if it does not exist.
    pub async fn head_bucket(&self, bucket_id: BucketId) -> Result<BucketInfo> {
        self.substrate_client
            .get_bucket_info(bucket_id)
            .await?
            .ok_or(S3ClientError::BucketNotFound(bucket_id))
    }

    /// List every bucket `account` (default: the signer) is a member of,
    /// owned or shared. The chain does not record which buckets contain S3
    /// objects, so this lists all of them.
    pub async fn list_buckets(&self, account: Option<AccountId32>) -> Result<Vec<BucketInfo>> {
        let account = account.unwrap_or_else(|| self.substrate_client.account());
        self.substrate_client.list_member_buckets(&account).await
    }

    /// Upload an object. The provider stores the data, commits it to the
    /// bucket, and records the key, content type and user metadata in its
    /// S3 index. Nothing about the object goes on chain.
    pub async fn put_object(
        &self,
        bucket_id: BucketId,
        key: &str,
        data: &[u8],
        options: PutObjectOptions,
    ) -> Result<PutObjectResponse> {
        info!(
            "Uploading object: {}/{} ({} bytes)",
            bucket_id,
            key,
            data.len()
        );
        validate_object_key(key)?;

        let content_type = options
            .content_type
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let mut req = self
            .http
            .put(self.object_url(bucket_id))
            .query(&[("key", key)])
            .header("content-type", content_type)
            .body(data.to_vec());
        for (k, v) in &options.metadata {
            req = req.header(format!("{USER_METADATA_HEADER_PREFIX}{k}"), v);
        }

        let response = self.send(req, "PUT", bucket_id, key).await?;
        let body: PutObjectWire = response.json().await?;
        let cid = parse_h256(&body.data_root)?;

        info!(
            "Object uploaded: {}/{} (etag={})",
            bucket_id, key, body.etag
        );
        Ok(PutObjectResponse {
            etag: body.etag,
            cid,
            size: body.size,
        })
    }

    /// Download an object by key.
    ///
    /// Unverified: the key-to-content mapping comes from the provider's
    /// index, which nothing on chain commits to (#410).
    pub async fn get_object(&self, bucket_id: BucketId, key: &str) -> Result<GetObjectResponse> {
        info!("Downloading object: {}/{}", bucket_id, key);
        validate_object_key(key)?;

        let req = self
            .http
            .get(self.object_url(bucket_id))
            .query(&[("key", key)]);
        let response = self.send(req, "GET", bucket_id, key).await?;

        let headers = response.headers().clone();
        let data = response.bytes().await?.to_vec();

        info!(
            "Object downloaded: {}/{} ({} bytes)",
            bucket_id,
            key,
            data.len()
        );
        Ok(GetObjectResponse {
            content_type: content_type(&headers),
            etag: header_str(&headers, "etag"),
            size: data.len() as u64,
            last_modified: header_u64(&headers, "last-modified"),
            metadata: user_metadata(&headers),
            data,
        })
    }

    /// Read an object's metadata from the provider's index without the data.
    pub async fn head_object(&self, bucket_id: BucketId, key: &str) -> Result<HeadObjectResponse> {
        validate_object_key(key)?;
        let req = self
            .http
            .head(self.object_url(bucket_id))
            .query(&[("key", key)]);
        let response = self.send(req, "HEAD", bucket_id, key).await?;
        let headers = response.headers();

        Ok(HeadObjectResponse {
            content_type: content_type(headers),
            etag: header_str(headers, "etag"),
            size: header_u64(headers, "content-length"),
            last_modified: header_u64(headers, "last-modified"),
            cid: parse_h256(&header_str(headers, "x-amz-data-root"))?,
            metadata: user_metadata(headers),
        })
    }

    /// Delete an object from the provider's index. The data remains in the
    /// bucket's committed history. Deleting a missing key succeeds.
    pub async fn delete_object(&self, bucket_id: BucketId, key: &str) -> Result<()> {
        info!("Deleting object: {}/{}", bucket_id, key);
        validate_object_key(key)?;

        let req = self
            .http
            .delete(self.object_url(bucket_id))
            .query(&[("key", key)]);
        self.send(req, "DELETE", bucket_id, key).await?;

        info!("Object deleted: {}/{}", bucket_id, key);
        Ok(())
    }

    /// List objects in a bucket from the provider's index.
    pub async fn list_objects_v2(
        &self,
        bucket_id: BucketId,
        params: ListObjectsParams,
    ) -> Result<ListObjectsResponse> {
        debug!("Listing objects in bucket: {}", bucket_id);

        let mut query: Vec<(&str, String)> = Vec::new();
        let optional = [
            ("prefix", params.prefix),
            ("delimiter", params.delimiter),
            ("start_after", params.start_after),
            ("continuation_token", params.continuation_token),
            ("max_keys", params.max_keys.map(|n| n.to_string())),
        ];
        for (name, value) in optional {
            if let Some(value) = value {
                query.push((name, value));
            }
        }

        let req = self
            .http
            .get(format!("{}/s3/{bucket_id}/objects", self.provider_url))
            .query(&query);
        let response = self.send(req, "GET", bucket_id, "").await?;
        Ok(response.json().await?)
    }

    fn object_url(&self, bucket_id: BucketId) -> String {
        format!("{}/s3/{bucket_id}/object", self.provider_url)
    }

    /// Sign `req` with the provider auth header, send it, and map error
    /// statuses to [`S3ClientError`]. `key` is used only in error values;
    /// pass `""` for requests that are not about one object.
    async fn send(
        &self,
        req: reqwest::RequestBuilder,
        method: &str,
        bucket_id: BucketId,
        key: &str,
    ) -> Result<Response> {
        let keypair = self.signer.keypair();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| S3ClientError::InternalError(format!("System clock error: {e}")))?
            .as_secs();
        let auth = provider_auth::build_auth_header(
            &keypair.public_key().0,
            method,
            bucket_id,
            timestamp,
            |msg| keypair.sign(msg).0,
        );

        let response = req.header("Authorization", auth).send().await?;
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(S3ClientError::AccessDenied),
            status => {
                let body = response.text().await.unwrap_or_default();
                if status == StatusCode::NOT_FOUND {
                    if let Some(err) = map_not_found(method, &body, bucket_id, key) {
                        return Err(err);
                    }
                }
                Err(S3ClientError::ProviderError(format!(
                    "{method} failed: {status} {body}"
                )))
            }
        }
    }
}

/// Reject empty keys and keys over 1024 bytes. The provider rejects only
/// empty keys; the 1024-byte limit is this client's.
fn validate_object_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_OBJECT_KEY_LEN {
        return Err(S3ClientError::InvalidObjectKey(key.to_string()));
    }
    Ok(())
}

/// Parse a `0x`-prefixed 32-byte hex string.
fn parse_h256(s: &str) -> Result<H256> {
    let bytes = hex::decode(s.trim_start_matches("0x"))
        .map_err(|e| S3ClientError::ProviderError(format!("Invalid hash {s:?}: {e}")))?;
    if bytes.len() != 32 {
        return Err(S3ClientError::ProviderError(format!(
            "Invalid hash {s:?}: expected 32 bytes"
        )));
    }
    Ok(H256::from_slice(&bytes))
}

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn header_u64(headers: &reqwest::header::HeaderMap, name: &str) -> u64 {
    header_str(headers, name).parse().unwrap_or_default()
}

fn content_type(headers: &reqwest::header::HeaderMap) -> String {
    let value = header_str(headers, "content-type");
    if value.is_empty() {
        "application/octet-stream".to_string()
    } else {
        value
    }
}

fn user_metadata(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let key = name.as_str().strip_prefix(USER_METADATA_HEADER_PREFIX)?;
            Some((key.to_string(), value.to_str().ok()?.to_string()))
        })
        .collect()
}

/// Map a provider 404 to a typed error from the `error` field of its JSON
/// body. A HEAD response has no body, so an empty HEAD 404 for a key maps to
/// a missing object; HEAD cannot tell that apart from an unknown route.
/// Returns `None` for any other 404 (a missing chunk, an unknown route),
/// which the caller reports as a provider error.
fn map_not_found(
    method: &str,
    body: &str,
    bucket_id: BucketId,
    key: &str,
) -> Option<S3ClientError> {
    let error = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_owned));
    let object_not_found = || S3ClientError::ObjectNotFound {
        bucket_id,
        key: key.to_string(),
    };
    match error.as_deref() {
        Some("bucket_not_found") => Some(S3ClientError::BucketNotFound(bucket_id)),
        Some("object_not_found") => Some(object_not_found()),
        None if method == "HEAD" && body.is_empty() && !key.is_empty() => Some(object_not_found()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_not_found_reads_the_provider_error_code() {
        let bucket = r#"{"error":"bucket_not_found","details":{"bucket_id":7}}"#;
        let object = r#"{"error":"object_not_found","details":{"bucket_id":7,"key":"k"}}"#;
        assert!(matches!(
            map_not_found("GET", bucket, 7, "k"),
            Some(S3ClientError::BucketNotFound(7))
        ));
        assert!(matches!(
            map_not_found("GET", object, 7, "k"),
            Some(S3ClientError::ObjectNotFound { bucket_id: 7, ref key }) if key == "k"
        ));
        assert!(matches!(
            map_not_found("HEAD", "", 7, "k"),
            Some(S3ClientError::ObjectNotFound { bucket_id: 7, .. })
        ));
        // A missing chunk or an unknown route is not a missing key.
        assert!(map_not_found("GET", r#"{"error":"not_found"}"#, 7, "k").is_none());
        assert!(map_not_found("GET", r#"{"error":"root_not_found"}"#, 7, "k").is_none());
        assert!(map_not_found("GET", "", 7, "k").is_none());
    }

    #[test]
    fn test_put_object_options_default() {
        let options = PutObjectOptions::default();
        assert!(options.content_type.is_none());
        assert!(options.metadata.is_empty());
    }

    #[test]
    fn validate_object_key_rejects_empty_and_too_long() {
        assert!(validate_object_key("a").is_ok());
        assert!(validate_object_key(&"a".repeat(MAX_OBJECT_KEY_LEN)).is_ok());
        assert!(validate_object_key("").is_err());
        assert!(validate_object_key(&"a".repeat(MAX_OBJECT_KEY_LEN + 1)).is_err());
    }

    #[test]
    fn parse_h256_accepts_prefixed_hex_and_rejects_wrong_length() {
        let hash = H256::repeat_byte(0xab);
        let hex = format!("0x{}", hex::encode(hash.as_bytes()));
        assert_eq!(parse_h256(&hex).unwrap(), hash);
        assert!(parse_h256("0xabcd").is_err());
        assert!(parse_h256("not hex").is_err());
    }

    #[test]
    fn user_metadata_reads_only_prefixed_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-amz-meta-color", "blue".parse().unwrap());
        headers.insert("content-type", "text/plain".parse().unwrap());
        let metadata = user_metadata(&headers);
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata.get("color").map(String::as_str), Some("blue"));
    }

    #[test]
    fn list_objects_response_decodes_provider_wire_format() {
        let json = r#"{
            "contents": [{"key": "a.txt", "size": 3, "last_modified": 10, "etag": "0x01"}],
            "common_prefixes": ["dir/"],
            "is_truncated": false,
            "next_continuation_token": null,
            "key_count": 1
        }"#;
        let parsed: ListObjectsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.contents[0].key, "a.txt");
        assert_eq!(parsed.common_prefixes, vec!["dir/".to_string()]);
        assert_eq!(parsed.key_count, 1);
    }
}
