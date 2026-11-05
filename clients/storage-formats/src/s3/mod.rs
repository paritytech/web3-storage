// SPDX-License-Identifier: Apache-2.0

//! S3-style objects in a bucket's file tree.
//!
//! An object is a file of the [`Tree`]: key `k` is the file at path `/k`.
//! [`Objects::put_object`] creates the directories a key needs and
//! [`Objects::delete_object`] removes the directories it leaves empty, so the
//! bucket has no directory objects. The content type and the user metadata
//! (keys lowercased) are stored in the file's manifest. The ETag is `0x` and
//! the hex of the content root.
//!
//! A key is 1 to 1024 bytes, split on `/` into segments that follow
//! [`validate_entry_name`]. A key cannot also be a directory of another key:
//! `a` and `a/b` cannot both exist ([`S3ClientError::KeyConflict`]).
//!
//! The limits of the [`crate::tree`] module apply.

use crate::format::{
    validate_entry_name, DirectoryEntry, MaxMetadataEntries, MetadataEntry, DEFAULT_MIME_TYPE,
};
use crate::tree::{BlobStore, EmptyParents, FileStat, Tree};
use crate::FsClientError;
use bounded_collections::Get;
use primitive_types::H256;
use std::collections::HashMap;
use storage_primitives::BucketId;
use thiserror::Error;

/// Maximum object key length in bytes.
const MAX_OBJECT_KEY_LEN: usize = 1024;

/// Default and maximum page size of [`Objects::list_objects_v2`].
const DEFAULT_MAX_KEYS: u32 = 1000;

/// Errors of the object operations.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum S3ClientError {
    /// The bucket has no object with this key.
    #[error("Object not found: {bucket_id}/{key}")]
    ObjectNotFound {
        /// The bucket that was read.
        bucket_id: BucketId,
        /// The requested key.
        key: String,
    },

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

    /// Reading or writing the bucket's file tree failed.
    #[error(transparent)]
    FileSystem(#[from] FsClientError),
}

/// Result type of the object operations.
pub type Result<T> = std::result::Result<T, S3ClientError>;

/// Options for [`Objects::put_object`].
#[derive(Default, Clone, Debug)]
pub struct PutObjectOptions {
    /// Content type (MIME type). `None` or an empty string stores
    /// `application/octet-stream`.
    pub content_type: Option<String>,
    /// User-defined metadata. Keys are stored lowercase.
    pub metadata: HashMap<String, String>,
}

/// Response from [`Objects::put_object`].
#[derive(Clone, Debug)]
pub struct PutObjectResponse {
    /// ETag: `0x` + hex of the content root.
    pub etag: String,
    /// Content root (Layer 0 data root) of the object data.
    pub cid: H256,
    /// Size of the uploaded object in bytes.
    pub size: u64,
}

/// Response from [`Objects::get_object`].
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

/// Response from [`Objects::head_object`].
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

/// Parameters for [`Objects::list_objects_v2`].
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
    /// Not set: listing reads no manifests. [`Objects::head_object`]
    /// returns the ETag.
    pub etag: Option<String>,
}

/// Response from [`Objects::list_objects_v2`].
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

/// The objects of one bucket, stored in its file tree.
pub struct Objects<S> {
    tree: Tree<S>,
}

impl<S: BlobStore> Objects<S> {
    /// The objects stored in `tree`.
    pub fn new(tree: Tree<S>) -> Self {
        Self { tree }
    }

    /// Upload an object. Replaces an existing object with the same key.
    pub async fn put_object(
        &self,
        key: &str,
        data: &[u8],
        options: PutObjectOptions,
    ) -> Result<PutObjectResponse> {
        let path = key_to_path(key)?;
        let metadata = to_metadata_entries(options.metadata)?;
        let content_type = options
            .content_type
            .unwrap_or_else(|| DEFAULT_MIME_TYPE.to_string());
        let (_, stat) = self
            .tree
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

    /// Download an object by key. The data is checked against the object's
    /// content root.
    pub async fn get_object(&self, key: &str) -> Result<GetObjectResponse> {
        let path = key_to_path(key)?;
        let file = self
            .tree
            .get_file(&path)
            .await
            .map_err(|e| self.object_error(e, key))?;
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

    /// Read an object's metadata without the data.
    pub async fn head_object(&self, key: &str) -> Result<HeadObjectResponse> {
        let path = key_to_path(key)?;
        let stat = self
            .tree
            .stat(&path)
            .await
            .map_err(|e| self.object_error(e, key))?;
        Ok(to_head(&stat))
    }

    /// Delete an object, and the prefix directories it leaves empty. The data
    /// remains in the bucket's committed history. Deleting a missing key
    /// succeeds.
    pub async fn delete_object(&self, key: &str) -> Result<()> {
        let path = key_to_path(key)?;
        match self.tree.delete(&path, EmptyParents::Remove).await {
            Ok(_) => Ok(()),
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

    /// List objects, sorted by key bytes.
    ///
    /// Reads the root and the deepest directory the prefix names. With
    /// delimiter `/`, reads each of its subdirectories only until it finds an
    /// object, unless `start_after` is a key inside one of them. With any
    /// other delimiter or none, reads every directory under it. Reads no
    /// manifests, so `etag` is not set.
    pub async fn list_objects_v2(&self, params: ListObjectsParams) -> Result<ListObjectsResponse> {
        let prefix = params.prefix.unwrap_or_default();
        let delimiter = params.delimiter.as_deref().filter(|d| !d.is_empty());
        let start_after = params.start_after.as_deref();

        // `a/b/c` lists under `/a/b`; `a/b/` lists under `/a/b`; `a` lists
        // under `/`.
        let dir_key = prefix.rfind('/').map_or("", |i| &prefix[..i]);
        let key_prefix = if dir_key.is_empty() {
            String::new()
        } else {
            format!("{dir_key}/")
        };
        // With delimiter `/`, each subdirectory of the listed directory that
        // contains an object is one common prefix. If `start_after` is a key
        // inside a subdirectory, the page shows that common prefix only if the
        // subdirectory has a later key, so read every directory.
        let start_after_in_subdir = start_after
            .and_then(|after| after.strip_prefix(key_prefix.as_str()))
            .is_some_and(|rel| rel.find('/').is_some_and(|i| i + 1 < rel.len()));
        let dir_only = delimiter == Some("/") && !start_after_in_subdir;

        let dir_path = if dir_key.is_empty() {
            Ok("/".to_string())
        } else {
            key_to_path(dir_key)
        };
        let mut objects: Vec<(String, DirectoryEntry)> = match dir_path {
            Ok(dir_path) => {
                let listed = if dir_only {
                    // A subdirectory gets the key `name/`, which `list_page`
                    // turns into a common prefix.
                    self.tree.list_non_empty(&dir_path).await.map(|entries| {
                        entries
                            .into_iter()
                            .map(|entry| {
                                let name = entry.name_str();
                                let rel = if entry.is_directory() {
                                    format!("{name}/")
                                } else {
                                    name
                                };
                                (rel, entry)
                            })
                            .collect()
                    })
                } else {
                    self.tree.files_under(&dir_path).await
                };
                match listed {
                    Ok(entries) => entries
                        .into_iter()
                        .map(|(rel, entry)| (format!("{key_prefix}{rel}"), entry))
                        .collect(),
                    Err(FsClientError::PathNotFound(_) | FsClientError::NotADirectory(_)) => {
                        Vec::new()
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            // No valid key starts with an invalid directory key.
            Err(_) => Vec::new(),
        };

        objects.sort_by(|a, b| a.0.cmp(&b.0));
        let keys: Vec<&str> = objects.iter().map(|(k, _)| k.as_str()).collect();
        let max_keys = params
            .max_keys
            .unwrap_or(DEFAULT_MAX_KEYS)
            .clamp(1, DEFAULT_MAX_KEYS) as usize;
        let page = list_page(&keys, &prefix, delimiter, start_after, max_keys);

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

    /// Map a tree read error for `key` to the S3 error.
    fn object_error(&self, e: FsClientError, key: &str) -> S3ClientError {
        match e {
            FsClientError::PathNotFound(_)
            | FsClientError::NotADirectory(_)
            | FsClientError::NotAFile(_) => S3ClientError::ObjectNotFound {
                bucket_id: self.tree.bucket_id(),
                key: key.to_string(),
            },
            other => other.into(),
        }
    }
}

/// Check `key` and return its file path, `/` + `key`.
fn key_to_path(key: &str) -> Result<String> {
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

fn etag(content_root: H256) -> String {
    format!("{content_root:#x}")
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
    let max_entries = MaxMetadataEntries::get() as usize;
    if lowered.len() > max_entries {
        return Err(S3ClientError::InvalidMetadata(format!(
            "{} entries, at most {max_entries}",
            lowered.len()
        )));
    }
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
mod tests;
