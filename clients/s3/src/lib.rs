// SPDX-License-Identifier: Apache-2.0

//! S3-compatible client for Web3 Storage.
//!
//! An S3 bucket is a Layer 0 bucket of `pallet-storage-provider`, identified
//! by its bucket id. The chain stores no bucket name and no object metadata.
//!
//! Objects are files in the bucket's file tree ([`file_system_client::tree`]):
//! key `k` is the file at path `/k`. The client reads and writes the tree
//! through Layer 0 routes only (`PUT /node`, `POST /commit`, `GET /read`,
//! `GET /node`, `GET /commitment`, `GET /mmr_proof`). Downloads are checked against the
//! object's content root, and the tree's root is the bucket's last MMR leaf.
//!
//! Limits:
//! - **Single writer.** Two clients that write the same bucket at the same
//!   time can lose a change: the last commit wins.
//! - **Only the S3 and file system clients may write the bucket.** Any other
//!   commit replaces the tree's root.
//! - **Prefix deletes.** An Admin `POST /delete` that moves `start_seq` past
//!   the leaves of blobs the current tree still references lets the provider
//!   drop those blobs.
//! - **Reads are unauthenticated.** Anyone who knows a CID can read the blob
//!   (#383, #396). Confidential data needs client-side encryption.
//! - A key cannot also be a prefix directory of another key: `a` and `a/b`
//!   cannot both exist ([`S3ClientError::KeyConflict`]).
//! - No bucket deletion: Layer 0 has no bucket deletion.

mod substrate;

pub use storage_client::Signer;
pub use storage_primitives::{BucketId, Role, Visibility};
pub use substrate::SubstrateClient;

use file_system_client::{BlobStore, EmptyParents, FileStat, FsClientError, Tree};
use file_system_primitives::{validate_entry_name, MetadataEntry, DEFAULT_MIME_TYPE};
use sp_core::H256;
use sp_runtime::AccountId32;
use std::collections::HashMap;
use storage_client::{ClientConfig, StorageUserClient};
use thiserror::Error;
use tracing::{debug, info};

/// Maximum object key length in bytes.
const MAX_OBJECT_KEY_LEN: usize = 1024;

/// Default and maximum page size of [`S3Client::list_objects_v2`].
const DEFAULT_MAX_KEYS: u32 = 1000;

/// S3 client error types.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum S3ClientError {
    /// No Layer 0 bucket with this id exists.
    #[error("Bucket not found: {0}")]
    BucketNotFound(BucketId),

    /// The bucket has no object with this key.
    #[error("Object not found: {bucket_id}/{key}")]
    ObjectNotFound { bucket_id: BucketId, key: String },

    /// The key is empty, longer than 1024 bytes, has an empty segment
    /// (leading, trailing or double `/`), a segment over 256 bytes, or a `.`
    /// or `..` segment.
    #[error("Invalid object key: {0}")]
    InvalidObjectKey(String),

    /// The key is the prefix directory of other keys, or a prefix of the key
    /// is itself a key. Delete the other object first.
    #[error("Object key conflicts with an existing key: {0}")]
    KeyConflict(String),

    /// Two metadata keys are equal after lowercasing, or a key or value is
    /// over the size bounds (64 entries, 64-byte keys, 256-byte values).
    #[error("Invalid user metadata: {0}")]
    InvalidMetadata(String),

    #[error("Chain error: {0}")]
    ChainError(String),

    /// Reading or writing the bucket's file tree failed.
    #[error(transparent)]
    FileSystem(#[from] FsClientError),
}

/// Result type for S3 client operations.
pub type Result<T> = std::result::Result<T, S3ClientError>;

/// Options for [`S3Client::put_object`].
#[derive(Default, Clone, Debug)]
pub struct PutObjectOptions {
    /// Content type (MIME type). `None` or an empty string stores
    /// `application/octet-stream`.
    pub content_type: Option<String>,
    /// User-defined metadata. Keys are stored lowercase.
    pub metadata: HashMap<String, String>,
}

/// Response from [`S3Client::put_object`].
#[derive(Clone, Debug)]
pub struct PutObjectResponse {
    /// ETag: `0x` + hex of the content root.
    pub etag: String,
    /// Content root (Layer 0 data root) of the object data.
    pub cid: H256,
    /// Size of the uploaded object in bytes.
    pub size: u64,
}

/// Response from [`S3Client::get_object`].
#[derive(Clone, Debug)]
pub struct GetObjectResponse {
    /// Object data, checked against the content root.
    pub data: Vec<u8>,
    /// Content type.
    pub content_type: String,
    /// ETag: `0x` + hex of the content root.
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
    /// ETag: `0x` + hex of the content root.
    pub etag: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modified time, in seconds since the Unix epoch.
    pub last_modified: u64,
    /// Content root (Layer 0 data root) of the object data.
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
    /// Return only keys after this key. To read the next page, pass the
    /// previous page's `next_start_after`.
    pub start_after: Option<String>,
    /// Maximum number of keys and common prefixes to return, 1 to 1000
    /// (default 1000). 0 counts as 1, so a truncated page always names
    /// `next_start_after`.
    pub max_keys: Option<u32>,
}

/// One object in a [`ListObjectsResponse`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectSummary {
    /// Object key.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modified time, in seconds since the Unix epoch.
    pub last_modified: u64,
    /// Not set: listing reads no manifests. [`S3Client::head_object`]
    /// returns the ETag.
    pub etag: Option<String>,
}

/// Response from [`S3Client::list_objects_v2`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListObjectsResponse {
    /// Matching objects, sorted by key bytes.
    pub contents: Vec<ObjectSummary>,
    /// Key prefixes grouped by the delimiter, sorted.
    pub common_prefixes: Vec<String>,
    /// `true` if more keys match than this page returns.
    pub is_truncated: bool,
    /// The last key or common prefix of this page when `is_truncated`; pass
    /// it as `start_after` for the next page.
    pub next_start_after: Option<String>,
    /// Number of keys and common prefixes returned.
    pub key_count: u32,
}

/// Object operations on a bucket's file tree, through one provider. Needs no
/// chain connection.
pub struct ObjectClient {
    storage: StorageUserClient,
}

impl ObjectClient {
    /// Create an object client for `provider_url`. `signer` authenticates
    /// uploads and commits; the provider checks its bucket role.
    pub fn new(provider_url: &str, signer: Signer) -> Result<Self> {
        let storage = StorageUserClient::new(
            ClientConfig {
                provider_urls: vec![provider_url.trim_end_matches('/').to_string()],
                ..Default::default()
            },
            signer,
        )
        .map_err(|e| FsClientError::Config(e.to_string()))?;
        Ok(Self { storage })
    }

    fn tree(&self, bucket_id: BucketId) -> Tree<&StorageUserClient> {
        Tree::new(&self.storage, bucket_id)
    }

    /// Upload an object. Replaces an existing object with the same key.
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
        put_object_in(&self.tree(bucket_id), key, data, options).await
    }

    /// Download an object by key. The data is checked against the object's
    /// content root.
    pub async fn get_object(&self, bucket_id: BucketId, key: &str) -> Result<GetObjectResponse> {
        info!("Downloading object: {}/{}", bucket_id, key);
        get_object_in(&self.tree(bucket_id), key).await
    }

    /// Read an object's metadata without the data.
    pub async fn head_object(&self, bucket_id: BucketId, key: &str) -> Result<HeadObjectResponse> {
        head_object_in(&self.tree(bucket_id), key).await
    }

    /// Delete an object, and the prefix directories it leaves empty. The data
    /// remains in the bucket's committed history. Deleting a missing key
    /// succeeds.
    pub async fn delete_object(&self, bucket_id: BucketId, key: &str) -> Result<()> {
        info!("Deleting object: {}/{}", bucket_id, key);
        let path = key_to_path(key)?;
        match self
            .tree(bucket_id)
            .delete(&path, EmptyParents::Remove)
            .await
        {
            Ok(()) => Ok(()),
            // No object with this key: the path is missing, passes through
            // a file, or is a directory.
            Err(
                FsClientError::PathNotFound(_)
                | FsClientError::NotADirectory(_)
                | FsClientError::NotAFile(_),
            ) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// List objects in a bucket, sorted by key bytes.
    ///
    /// Reads the root and every directory under the deepest directory the
    /// prefix names. Reads no manifests, so `etag` is not set.
    pub async fn list_objects_v2(
        &self,
        bucket_id: BucketId,
        params: ListObjectsParams,
    ) -> Result<ListObjectsResponse> {
        debug!("Listing objects in bucket: {}", bucket_id);
        let prefix = params.prefix.unwrap_or_default();
        let tree = self.tree(bucket_id);

        // `a/b/c` lists under `/a/b`; `a/b/` lists under `/a/b`; `a` lists
        // under `/`.
        let dir_key = prefix.rfind('/').map_or("", |i| &prefix[..i]);
        let objects = if dir_key.is_empty() {
            tree.files_under("/").await?
        } else {
            match key_to_path(dir_key) {
                Ok(dir_path) => match tree.files_under(&dir_path).await {
                    Ok(files) => files
                        .into_iter()
                        .map(|(rel, entry)| (format!("{dir_key}/{rel}"), entry))
                        .collect(),
                    Err(FsClientError::PathNotFound(_) | FsClientError::NotADirectory(_)) => {
                        Vec::new()
                    }
                    Err(e) => return Err(e.into()),
                },
                // No valid key starts with an invalid directory key.
                Err(_) => Vec::new(),
            }
        };

        let mut objects = objects;
        objects.sort_by(|a, b| a.0.cmp(&b.0));
        let keys: Vec<&str> = objects.iter().map(|(k, _)| k.as_str()).collect();
        let max_keys = params
            .max_keys
            .unwrap_or(DEFAULT_MAX_KEYS)
            .clamp(1, DEFAULT_MAX_KEYS) as usize;
        let page = list_page(
            &keys,
            &prefix,
            params.delimiter.as_deref().filter(|d| !d.is_empty()),
            params.start_after.as_deref(),
            max_keys,
        );

        let contents: Vec<ObjectSummary> = page
            .contents
            .iter()
            .map(|&i| {
                let (key, entry) = &objects[i];
                ObjectSummary {
                    key: key.clone(),
                    size: entry.size,
                    last_modified: entry.mtime,
                    etag: None,
                }
            })
            .collect();

        Ok(ListObjectsResponse {
            key_count: (contents.len() + page.common_prefixes.len()) as u32,
            contents,
            common_prefixes: page.common_prefixes,
            is_truncated: page.is_truncated,
            next_start_after: page.next_start_after,
        })
    }
}

/// S3 client: Layer 0 buckets on chain, objects through [`ObjectClient`].
pub struct S3Client {
    objects: ObjectClient,
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
            objects: ObjectClient::new(provider_url, signer)?,
            substrate_client,
        })
    }

    /// The object operations of this client.
    pub fn objects(&self) -> &ObjectClient {
        &self.objects
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

    /// See [`ObjectClient::put_object`].
    pub async fn put_object(
        &self,
        bucket_id: BucketId,
        key: &str,
        data: &[u8],
        options: PutObjectOptions,
    ) -> Result<PutObjectResponse> {
        self.objects.put_object(bucket_id, key, data, options).await
    }

    /// See [`ObjectClient::get_object`].
    pub async fn get_object(&self, bucket_id: BucketId, key: &str) -> Result<GetObjectResponse> {
        self.objects.get_object(bucket_id, key).await
    }

    /// See [`ObjectClient::head_object`].
    pub async fn head_object(&self, bucket_id: BucketId, key: &str) -> Result<HeadObjectResponse> {
        self.objects.head_object(bucket_id, key).await
    }

    /// See [`ObjectClient::delete_object`].
    pub async fn delete_object(&self, bucket_id: BucketId, key: &str) -> Result<()> {
        self.objects.delete_object(bucket_id, key).await
    }

    /// See [`ObjectClient::list_objects_v2`].
    pub async fn list_objects_v2(
        &self,
        bucket_id: BucketId,
        params: ListObjectsParams,
    ) -> Result<ListObjectsResponse> {
        self.objects.list_objects_v2(bucket_id, params).await
    }
}

/// Check `key` and return its file path, `/` + `key`.
///
/// A key is 1..=1024 bytes, split on `/` into segments of 1..=256 bytes, with
/// no empty, `.` or `..` segment.
pub fn key_to_path(key: &str) -> Result<String> {
    let valid = !key.is_empty()
        && key.len() <= MAX_OBJECT_KEY_LEN
        && key
            .split('/')
            .all(|segment| validate_entry_name(segment.as_bytes()).is_ok());
    if valid {
        Ok(format!("/{key}"))
    } else {
        Err(S3ClientError::InvalidObjectKey(key.to_string()))
    }
}

/// [`ObjectClient::put_object`] on `tree`.
async fn put_object_in<S: BlobStore>(
    tree: &Tree<S>,
    key: &str,
    data: &[u8],
    options: PutObjectOptions,
) -> Result<PutObjectResponse> {
    let path = key_to_path(key)?;
    let metadata = to_metadata_entries(options.metadata)?;
    let content_type = options
        .content_type
        .unwrap_or_else(|| DEFAULT_MIME_TYPE.to_string());
    let stat = tree
        .put_file(&path, data, &content_type, metadata)
        .await
        .map_err(|e| match e {
            FsClientError::NotAFile(_) | FsClientError::NotADirectory(_) => {
                S3ClientError::KeyConflict(key.to_string())
            }
            other => other.into(),
        })?;
    let cid = stat.content_root();
    Ok(PutObjectResponse {
        etag: etag(cid),
        cid,
        size: stat.size(),
    })
}

/// [`ObjectClient::get_object`] on `tree`.
async fn get_object_in<S: BlobStore>(tree: &Tree<S>, key: &str) -> Result<GetObjectResponse> {
    let path = key_to_path(key)?;
    let file = tree
        .get_file(&path)
        .await
        .map_err(|e| object_error(e, tree.bucket_id(), key))?;
    let head = to_head(&file.stat);
    Ok(GetObjectResponse {
        data: file.data,
        content_type: head.content_type,
        etag: head.etag,
        size: head.size,
        last_modified: head.last_modified,
        metadata: head.metadata,
    })
}

/// [`ObjectClient::head_object`] on `tree`.
async fn head_object_in<S: BlobStore>(tree: &Tree<S>, key: &str) -> Result<HeadObjectResponse> {
    let path = key_to_path(key)?;
    let stat = tree
        .stat(&path)
        .await
        .map_err(|e| object_error(e, tree.bucket_id(), key))?;
    Ok(to_head(&stat))
}

fn etag(content_root: H256) -> String {
    format!("0x{}", hex::encode(content_root.as_bytes()))
}

/// Map a tree read error for `key` to the S3 error.
fn object_error(e: FsClientError, bucket_id: BucketId, key: &str) -> S3ClientError {
    match e {
        FsClientError::PathNotFound(_)
        | FsClientError::NotADirectory(_)
        | FsClientError::NotAFile(_) => S3ClientError::ObjectNotFound {
            bucket_id,
            key: key.to_string(),
        },
        other => other.into(),
    }
}

fn to_head(stat: &FileStat) -> HeadObjectResponse {
    let cid = stat.content_root();
    HeadObjectResponse {
        content_type: stat.content_type(),
        etag: etag(cid),
        size: stat.size(),
        last_modified: stat.mtime(),
        cid,
        metadata: stat
            .manifest
            .user_metadata
            .iter()
            .map(|m| {
                (
                    String::from_utf8_lossy(&m.key).into_owned(),
                    String::from_utf8_lossy(&m.value).into_owned(),
                )
            })
            .collect(),
    }
}

/// Lowercase the keys and sort by key. Fails if two keys are equal after
/// lowercasing or an entry is over the bounds.
fn to_metadata_entries(metadata: HashMap<String, String>) -> Result<Vec<MetadataEntry>> {
    let mut lowered: Vec<(String, String)> = metadata
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();
    lowered.sort();
    if let Some(pair) = lowered.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(S3ClientError::InvalidMetadata(format!(
            "duplicate key {:?}",
            pair[0].0
        )));
    }
    lowered
        .iter()
        .map(|(k, v)| {
            MetadataEntry::try_new(k.as_bytes(), v.as_bytes())
                .map_err(|_| S3ClientError::InvalidMetadata(format!("key {k:?} or its value")))
        })
        .collect()
}

/// One page of a listing, as indices into the sorted keys.
#[derive(Debug, PartialEq, Eq)]
struct Page {
    contents: Vec<usize>,
    common_prefixes: Vec<String>,
    is_truncated: bool,
    next_start_after: Option<String>,
}

/// Select one page from `keys` (sorted by bytes): keys that start with
/// `prefix` and sort after `start_after`. With a `delimiter`, keys that
/// contain it after the prefix collapse into one common prefix (the key up to
/// and including the delimiter). A common prefix equal to `start_after` is
/// skipped, so `next_start_after` pages past it.
/// `max_keys` must be at least 1.
fn list_page(
    keys: &[&str],
    prefix: &str,
    delimiter: Option<&str>,
    start_after: Option<&str>,
    max_keys: usize,
) -> Page {
    let mut page = Page {
        contents: Vec::new(),
        common_prefixes: Vec::new(),
        is_truncated: false,
        next_start_after: None,
    };
    let mut last: Option<&str> = None;
    for (index, key) in keys.iter().enumerate() {
        if !key.starts_with(prefix) || start_after.is_some_and(|after| *key <= after) {
            continue;
        }
        let group = delimiter.and_then(|d| {
            key[prefix.len()..]
                .find(d)
                .map(|i| &key[..prefix.len() + i + d.len()])
        });
        if let Some(group) = group {
            if Some(group) == start_after || Some(group) == last {
                continue;
            }
        }
        if page.contents.len() + page.common_prefixes.len() == max_keys {
            page.is_truncated = true;
            break;
        }
        match group {
            Some(group) => {
                page.common_prefixes.push(group.to_string());
                last = Some(group);
            }
            None => {
                page.contents.push(index);
                last = Some(key);
            }
        }
    }
    if page.is_truncated {
        page.next_start_after = last.map(str::to_string);
    }
    page
}

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_rules() {
        assert_eq!(key_to_path("a").unwrap(), "/a");
        assert_eq!(key_to_path("a/b/c.txt").unwrap(), "/a/b/c.txt");
        assert!(key_to_path(&"a".repeat(256)).is_ok());
        let long_key = vec!["a".repeat(200); 5].join("/");
        assert_eq!(long_key.len(), 1004);
        assert!(key_to_path(&long_key).is_ok());

        let too_long = vec!["a".repeat(255); 5].join("/");
        assert!(too_long.len() > MAX_OBJECT_KEY_LEN);
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            ".",
            "..",
            "a/./b",
            "a/../b",
            &"a".repeat(257),
            &too_long,
        ] {
            assert!(
                matches!(key_to_path(bad), Err(S3ClientError::InvalidObjectKey(_))),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn metadata_keys_are_lowercased_sorted_and_unique() {
        let metadata = HashMap::from([
            ("Color".to_string(), "blue".to_string()),
            ("ALPHA".to_string(), "1".to_string()),
        ]);
        let entries = to_metadata_entries(metadata).unwrap();
        let keys: Vec<&[u8]> = entries.iter().map(|e| e.key.as_slice()).collect();
        assert_eq!(keys, [&b"alpha"[..], b"color"]);

        let clash = HashMap::from([
            ("Color".to_string(), "blue".to_string()),
            ("color".to_string(), "red".to_string()),
        ]);
        assert!(matches!(
            to_metadata_entries(clash),
            Err(S3ClientError::InvalidMetadata(_))
        ));
        let long_value = HashMap::from([("k".to_string(), "v".repeat(257))]);
        assert!(to_metadata_entries(long_value).is_err());
    }

    const KEYS: &[&str] = &[
        "a.txt",
        "b/1.txt",
        "b/2.txt",
        "b/c/3.txt",
        "b0.txt",
        "d/4.txt",
    ];

    fn keys_of(page: &Page) -> Vec<&'static str> {
        page.contents.iter().map(|&i| KEYS[i]).collect()
    }

    #[test]
    fn keys_sort_by_bytes() {
        // `b/` (0x2f) sorts before `b0` (0x30): S3 order, not tree walk order.
        let mut sorted = KEYS.to_vec();
        sorted.sort();
        assert_eq!(sorted, KEYS);
    }

    #[test]
    fn list_without_delimiter_returns_all_matching_keys() {
        let page = list_page(KEYS, "", None, None, 1000);
        assert_eq!(keys_of(&page), KEYS);
        assert!(!page.is_truncated);
        assert_eq!(page.next_start_after, None);

        let page = list_page(KEYS, "b/", None, None, 1000);
        assert_eq!(keys_of(&page), ["b/1.txt", "b/2.txt", "b/c/3.txt"]);

        let page = list_page(KEYS, "b", None, None, 1000);
        assert_eq!(
            keys_of(&page),
            ["b/1.txt", "b/2.txt", "b/c/3.txt", "b0.txt"]
        );
    }

    #[test]
    fn list_with_delimiter_groups_common_prefixes() {
        let page = list_page(KEYS, "", Some("/"), None, 1000);
        assert_eq!(keys_of(&page), ["a.txt", "b0.txt"]);
        assert_eq!(page.common_prefixes, ["b/", "d/"]);

        let page = list_page(KEYS, "b/", Some("/"), None, 1000);
        assert_eq!(keys_of(&page), ["b/1.txt", "b/2.txt"]);
        assert_eq!(page.common_prefixes, ["b/c/"]);
    }

    #[test]
    fn list_start_after_skips_keys_and_prefixes() {
        let page = list_page(KEYS, "", None, Some("b/2.txt"), 1000);
        assert_eq!(keys_of(&page), ["b/c/3.txt", "b0.txt", "d/4.txt"]);

        // A common prefix as start_after skips every key under it.
        let page = list_page(KEYS, "", Some("/"), Some("b/"), 1000);
        assert_eq!(keys_of(&page), ["b0.txt"]);
        assert_eq!(page.common_prefixes, ["d/"]);
    }

    #[test]
    fn list_max_keys_truncates_and_pages() {
        let first = list_page(KEYS, "", Some("/"), None, 2);
        assert_eq!(keys_of(&first), ["a.txt"]);
        assert_eq!(first.common_prefixes, ["b/"]);
        assert!(first.is_truncated);
        assert_eq!(first.next_start_after.as_deref(), Some("b/"));

        let second = list_page(KEYS, "", Some("/"), first.next_start_after.as_deref(), 2);
        assert_eq!(keys_of(&second), ["b0.txt"]);
        assert_eq!(second.common_prefixes, ["d/"]);
        assert!(!second.is_truncated);
        assert_eq!(second.next_start_after, None);

        let exact = list_page(KEYS, "", None, None, KEYS.len());
        assert!(!exact.is_truncated);
    }
}
