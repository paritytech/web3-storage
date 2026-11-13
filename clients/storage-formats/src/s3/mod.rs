// SPDX-License-Identifier: Apache-2.0

//! S3-style objects in a bucket's file tree.
//!
//! An object is a file of the [`Tree`]: key `k` is the file at path `/k`.
//! [`S3Bucket::put_object`] creates the directories a key needs and
//! [`S3Bucket::delete_object`] removes the directories it leaves empty, so the
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
    validate_entry_name, DirectoryEntry, FileSystemError, MaxMetadataEntries, MetadataEntry,
    DEFAULT_MIME_TYPE,
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

/// Default and maximum page size of [`S3Bucket::list_objects`].
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

/// Options for [`S3Bucket::put_object`].
#[derive(Default, Clone, Debug)]
pub struct PutObjectOptions {
    /// Content type (MIME type). `None` or an empty string stores
    /// `application/octet-stream`.
    pub content_type: Option<String>,
    /// User-defined metadata. Keys are stored lowercase.
    pub metadata: HashMap<String, String>,
}

/// Response from [`S3Bucket::put_object`].
#[derive(Clone, Debug)]
pub struct PutObjectResponse {
    /// ETag: `0x` + hex of the content root.
    pub etag: String,
    /// Content root (data root) of the object data.
    pub cid: H256,
    /// Size of the uploaded object in bytes.
    pub size: u64,
}

/// Response from [`S3Bucket::get_object`].
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

/// Response from [`S3Bucket::head_object`].
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
    /// Content root (data root) of the object data.
    pub cid: H256,
    /// User metadata.
    pub metadata: HashMap<String, String>,
}

/// Parameters for [`S3Bucket::list_objects`].
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
}

/// Response from [`S3Bucket::list_objects`].
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
}

/// The objects of one bucket, stored in its file tree.
pub struct S3Bucket<S> {
    tree: Tree<S>,
}

impl<S: BlobStore> S3Bucket<S> {
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
                FsClientError::Format {
                    rule: FileSystemError::DuplicateMetadataKey,
                    ..
                } => S3ClientError::InvalidMetadata("two keys are equal after lowercasing".into()),
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
    /// Walks the deepest directory the prefix names in key order and stops
    /// once the page is full. Skips every subtree that cannot change the
    /// page: one outside the prefix, one that sorts before `start_after`, and
    /// one whose keys all fall in a common prefix already on the page (with
    /// delimiter `/`, each subdirectory is read only until it shows an
    /// object). Reads no manifests: [`S3Bucket::head_object`] returns an
    /// object's ETag.
    pub async fn list_objects(&self, params: ListObjectsParams) -> Result<ListObjectsResponse> {
        let prefix = params.prefix.unwrap_or_default();
        let delimiter = params.delimiter.as_deref().filter(|d| !d.is_empty());
        let max_keys = params
            .max_keys
            .unwrap_or(DEFAULT_MAX_KEYS)
            .clamp(1, DEFAULT_MAX_KEYS) as usize;
        let mut page =
            PageBuilder::new(&prefix, delimiter, params.start_after.as_deref(), max_keys);

        // `a/b/c` lists under `/a/b`; `a/b/` lists under `/a/b`; `a` lists
        // under `/`.
        let dir_key = prefix.rfind('/').map_or("", |i| &prefix[..i]);
        let base = if dir_key.is_empty() {
            self.tree.list("/").await
        } else {
            match key_to_path(dir_key) {
                Ok(path) => self.tree.list(&path).await,
                // No valid key starts with an invalid directory key.
                Err(_) => Ok(Vec::new()),
            }
        };
        let base = match base {
            Ok(entries) => entries,
            Err(FsClientError::PathNotFound(_) | FsClientError::NotADirectory(_)) => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let key_prefix = if dir_key.is_empty() {
            String::new()
        } else {
            format!("{dir_key}/")
        };

        let mut stack = vec![(key_prefix, in_key_order(base))];
        while let Some((dir, entries)) = stack.last_mut() {
            let Some(entry) = entries.next() else {
                stack.pop();
                continue;
            };
            let key = format!("{dir}{}", entry.name_str());
            if entry.is_file() {
                let summary = || ObjectSummary {
                    key: key.clone(),
                    size: entry.size,
                    last_modified: entry.mtime,
                };
                if !page.push(&key, summary) {
                    break;
                }
                continue;
            }
            let sub = format!("{key}/");
            if page.skips(&sub) {
                continue;
            }
            let node = self.tree.read_dir(&format!("/{key}"), entry.cid).await?;
            stack.push((sub, in_key_order(node.children.into_inner())));
        }

        let page = page.finish();
        Ok(ListObjectsResponse {
            contents: page.contents,
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

/// Lowercase the keys. Fails if there are too many entries or one is over
/// the bounds.
fn to_metadata_entries(metadata: HashMap<String, String>) -> Result<Vec<MetadataEntry>> {
    let lowered: Vec<(String, String)> = metadata
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();
    let max_entries = MaxMetadataEntries::get() as usize;
    if lowered.len() > max_entries {
        return Err(S3ClientError::InvalidMetadata(format!(
            "{} entries, at most {max_entries}",
            lowered.len()
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

/// `entries` in key order: a file sorts by its name, a directory by its
/// name and `/`, the order of the keys under it.
fn in_key_order(mut entries: Vec<DirectoryEntry>) -> std::vec::IntoIter<DirectoryEntry> {
    entries.sort_by_cached_key(|entry| {
        let mut key = entry.name.to_vec();
        if entry.is_directory() {
            key.push(b'/');
        }
        key
    });
    entries.into_iter()
}

/// One page of a listing.
#[derive(Debug, PartialEq, Eq)]
struct Page<T> {
    contents: Vec<T>,
    common_prefixes: Vec<String>,
    is_truncated: bool,
    next_start_after: Option<String>,
}

/// Builds one page from keys added in key order (sorted by bytes): keys that
/// start with `prefix` and sort after `start_after`. With a `delimiter`, keys
/// that contain it after the prefix collapse into one common prefix (the key
/// up to and including the delimiter). A common prefix equal to `start_after`
/// is skipped, so `next_start_after` pages past it. At most `max_keys` (at
/// least 1) keys and common prefixes.
struct PageBuilder<'a, T> {
    prefix: &'a str,
    delimiter: Option<&'a str>,
    start_after: Option<&'a str>,
    max_keys: usize,
    page: Page<T>,
    last: Option<String>,
}

impl<'a, T> PageBuilder<'a, T> {
    fn new(
        prefix: &'a str,
        delimiter: Option<&'a str>,
        start_after: Option<&'a str>,
        max_keys: usize,
    ) -> Self {
        Self {
            prefix,
            delimiter,
            start_after,
            max_keys,
            page: Page {
                contents: Vec::new(),
                common_prefixes: Vec::new(),
                is_truncated: false,
                next_start_after: None,
            },
            last: None,
        }
    }

    /// The common prefix of `key`, if `key` has the delimiter after the
    /// prefix.
    fn group<'k>(&self, key: &'k str) -> Option<&'k str> {
        let d = self.delimiter?;
        let rel = key.get(self.prefix.len()..)?;
        rel.find(d).map(|i| &key[..self.prefix.len() + i + d.len()])
    }

    /// Add the next key, with the item `item` makes for it. Returns `false`
    /// once the page is full: no later key fits on it.
    fn push(&mut self, key: &str, item: impl FnOnce() -> T) -> bool {
        if !key.starts_with(self.prefix) || self.start_after.is_some_and(|after| key <= after) {
            return true;
        }
        let group = self.group(key);
        if group.is_some_and(|g| Some(g) == self.start_after || Some(g) == self.last.as_deref()) {
            return true;
        }
        if self.page.contents.len() + self.page.common_prefixes.len() == self.max_keys {
            self.page.is_truncated = true;
            return false;
        }
        match group {
            Some(group) => {
                self.page.common_prefixes.push(group.to_string());
                self.last = Some(group.to_string());
            }
            None => {
                self.page.contents.push(item());
                self.last = Some(key.to_string());
            }
        }
        true
    }

    /// Whether no key that starts with `dir` (a key prefix ending in `/`)
    /// can change the page.
    fn skips(&self, dir: &str) -> bool {
        if !dir.starts_with(self.prefix) && !self.prefix.starts_with(dir) {
            return true;
        }
        // `after` sorts after every key that starts with `dir`.
        if self
            .start_after
            .is_some_and(|after| after > dir && !after.starts_with(dir))
        {
            return true;
        }
        // Every key under `dir` falls in the common prefix of `dir`.
        self.group(dir)
            .is_some_and(|g| Some(g) == self.start_after || Some(g) == self.last.as_deref())
    }

    fn finish(mut self) -> Page<T> {
        if self.page.is_truncated {
            self.page.next_start_after = self.last;
        }
        self.page
    }
}

/// One page of `keys` (sorted by bytes), as indices into `keys`; see
/// [`PageBuilder`].
#[cfg(test)]
fn list_page(
    keys: &[&str],
    prefix: &str,
    delimiter: Option<&str>,
    start_after: Option<&str>,
    max_keys: usize,
) -> Page<usize> {
    let mut page = PageBuilder::new(prefix, delimiter, start_after, max_keys);
    for (index, key) in keys.iter().enumerate() {
        if !page.push(key, || index) {
            break;
        }
    }
    page.finish()
}

#[cfg(test)]
mod tests;
