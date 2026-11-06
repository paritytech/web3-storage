// SPDX-License-Identifier: Apache-2.0

//! The file tree of a drive, stored in its Layer 0 bucket.
//!
//! This module is the only code that knows the on-bucket format; the file
//! system client and the S3 client both use it.
//!
//! # Format
//!
//! Directories are [`DirectoryNode`] blobs and files are a content blob plus
//! a [`FileManifest`] blob. A node blob is a 6-byte header (magic `W3FS`,
//! format version, kind of node) and the SCALE encoding
//! ([`DirectoryNode::to_blob`]). Every blob is addressed by its CID, the
//! Layer 0 data root ([`compute_cid`]).
//!
//! # Root discovery
//!
//! The root directory is the data root of the bucket's last MMR leaf
//! (`GET /commitment`, then `GET /mmr_proof` for leaf `leaf_count - 1`,
//! checked to be the last leaf of the commitment's MMR). A bucket with no
//! leaves, or one the provider answers 404 for, is an empty drive: its root
//! is `DirectoryNode::new_empty(bucket_id)`, which is not stored until the
//! first write.
//!
//! # Writes
//!
//! Every write reads the current root, uploads the new blobs (content,
//! manifest, then each rewritten directory from the changed entry up to the
//! root), and commits all of them in one `POST /commit` with the new root
//! last. Directories off the changed path keep their CIDs.
//!
//! # Limits
//!
//! - **Single writer.** Two writers that both read root R and then commit
//!   each produce a root that lacks the other's change; the last commit wins.
//! - **Only these clients may write the bucket.** Any other commit becomes
//!   the last leaf, and root discovery then fails with
//!   [`FsClientError::NotAFileSystemBucket`].
//! - **Prefix deletes.** An Admin `POST /delete` that moves `start_seq` past
//!   the leaves of blobs the current tree still references lets the provider
//!   drop those blobs.
//! - **The provider is trusted for the root.** The client does not check the
//!   provider's signature on `GET /commitment`, so a provider can serve an
//!   older commitment.
//! - **Directory and manifest reads have no known size.** They read at most
//!   the type's maximum SCALE encoded size. A provider that returns the two
//!   child hashes of an internal Merkle node as a 64-byte blob, and denies
//!   the node's children on `GET /node`, is not detected.
//! - **Blob reads by CID are unauthenticated.** Anyone who knows a CID and a
//!   bucket id can read the blob (#383, #396). Root discovery
//!   (`GET /commitment`, `GET /mmr_proof`) needs Reader on a private bucket.
//!   Confidential data needs client-side encryption.

use crate::format::{
    compute_cid, Cid, DirectoryEntry, DirectoryNode, EntryType, FileManifest, FileSystemError,
    MetadataEntry,
};
use crate::{FsClientError, Result};
use std::collections::HashSet;
use storage_primitives::{verify_mmr_proof, BucketId, Commitment, MmrProof};

/// The length of a blob to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobLength {
    /// The blob has exactly this many bytes (file content: the manifest's
    /// `total_size`).
    Exact(u64),
    /// The blob size is unknown and is at most this many bytes (directory
    /// nodes and manifests: their maximum SCALE encoded size). A store reads
    /// up to one byte more, so that [`Tree`] can reject a longer blob.
    AtMost(u64),
}

/// The Layer 0 operations the tree uses. On wasm32 the store is a JS object,
/// so the trait has no `Send` bound there.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait BlobStore: StoreBounds {
    /// Upload `data` to the bucket (chunks and Merkle tree nodes) and
    /// return its data root.
    async fn put_blob(&self, bucket_id: BucketId, data: &[u8]) -> Result<Cid>;

    /// Read the blob with data root `cid`.
    async fn get_blob(&self, bucket_id: BucketId, cid: Cid, length: BlobLength) -> Result<Vec<u8>>;

    /// Append `data_roots` to the bucket's MMR, in order.
    async fn commit(&self, bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()>;

    /// The bucket's current MMR commitment, or `None` if the bucket has
    /// none.
    async fn commitment(&self, bucket_id: BucketId) -> Result<Option<Commitment>>;

    /// MMR leaf `leaf_index` of the bucket and its inclusion proof.
    async fn mmr_proof(&self, bucket_id: BucketId, leaf_index: u64) -> Result<MmrProof>;
}

/// `Send + Sync` except on wasm32, where JS values are neither.
#[cfg(not(target_arch = "wasm32"))]
pub trait StoreBounds: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> StoreBounds for T {}
/// `Send + Sync` except on wasm32, where JS values are neither.
#[cfg(target_arch = "wasm32")]
pub trait StoreBounds {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> StoreBounds for T {}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<T: BlobStore + ?Sized> BlobStore for &T {
    async fn put_blob(&self, bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
        (**self).put_blob(bucket_id, data).await
    }

    async fn get_blob(&self, bucket_id: BucketId, cid: Cid, length: BlobLength) -> Result<Vec<u8>> {
        (**self).get_blob(bucket_id, cid, length).await
    }

    async fn commit(&self, bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
        (**self).commit(bucket_id, data_roots).await
    }

    async fn commitment(&self, bucket_id: BucketId) -> Result<Option<Commitment>> {
        (**self).commitment(bucket_id).await
    }

    async fn mmr_proof(&self, bucket_id: BucketId, leaf_index: u64) -> Result<MmrProof> {
        (**self).mmr_proof(bucket_id, leaf_index).await
    }
}

/// The error for a write at `path` that a format rule rejects.
fn format_error(path: &str) -> impl Fn(FileSystemError) -> FsClientError + '_ {
    move |source| FsClientError::Format {
        path: path.to_string(),
        source,
    }
}

/// The error for a blob at `path` that breaks a format rule. A newer format
/// version is [`FsClientError::UnsupportedFormatVersion`].
fn invalid_blob(path: &str) -> impl Fn(FileSystemError) -> FsClientError + '_ {
    move |source| match source {
        FileSystemError::UnsupportedVersion(version) => {
            FsClientError::UnsupportedFormatVersion(version)
        }
        source => FsClientError::InvalidBlob {
            path: path.to_string(),
            reason: source.to_string(),
        },
    }
}

/// The current root directory of a drive.
#[derive(Clone, Debug)]
pub struct Root {
    /// CID of the root directory; `None` for a drive with no commits.
    pub cid: Option<Cid>,
    /// The root directory.
    pub node: DirectoryNode,
}

/// A file's directory entry and manifest.
#[derive(Clone, Debug)]
pub struct FileStat {
    /// The file's entry in its parent directory.
    pub entry: DirectoryEntry,
    /// The file's manifest.
    pub manifest: FileManifest,
}

impl FileStat {
    /// Content size in bytes.
    pub fn size(&self) -> u64 {
        self.manifest.total_size
    }

    /// Modification time, in seconds since the Unix epoch.
    pub fn mtime(&self) -> u64 {
        self.entry.mtime
    }

    /// Content type.
    pub fn content_type(&self) -> String {
        self.manifest.mime_type_str()
    }

    /// Data root of the content.
    pub fn content_root(&self) -> Cid {
        self.manifest.content_root()
    }
}

/// A file read with [`Tree::get_file`].
#[derive(Clone, Debug)]
pub struct FileContent {
    /// Entry and manifest.
    pub stat: FileStat,
    /// Content bytes, checked against the manifest's content root.
    pub data: Vec<u8>,
}

/// How [`Tree::delete`] treats directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyParents {
    /// File system semantics: delete a file or an empty directory, and keep
    /// the directories the delete leaves empty.
    Keep,
    /// S3 semantics (there are no directory objects): delete only a file
    /// (a directory fails with [`FsClientError::NotAFile`]), and remove the
    /// directories the delete leaves empty, up to, not including, the root.
    Remove,
}

/// Split an absolute path into its names. `/` is the root and has none.
///
/// A path starts with `/`, separates names with `/`, and has no empty name
/// (no trailing or double `/`). Each name follows
/// [`crate::format::validate_entry_name`].
pub fn parse_path(path: &str) -> Result<Vec<&str>> {
    let invalid = || FsClientError::InvalidPath(path.to_string());
    let rest = path.strip_prefix('/').ok_or_else(invalid)?;
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    rest.split('/')
        .map(|name| {
            crate::format::validate_entry_name(name.as_bytes())
                .map(|_| name)
                .map_err(|_| invalid())
        })
        .collect()
}

/// The absolute path of `names`.
fn join(names: &[&str]) -> String {
    format!("/{}", names.join("/"))
}

fn system_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The file tree of one bucket.
pub struct Tree<S> {
    store: S,
    bucket_id: BucketId,
    now: fn() -> u64,
}

impl<S: BlobStore> Tree<S> {
    /// The tree of `bucket_id`, read and written through `store`.
    pub fn new(store: S, bucket_id: BucketId) -> Self {
        Self::with_clock(store, bucket_id, system_clock)
    }

    /// Like [`Tree::new`], with `now` (unix seconds) as the clock for entry
    /// `mtime`s. Tests use a fixed clock to get reproducible CIDs.
    pub fn with_clock(store: S, bucket_id: BucketId, now: fn() -> u64) -> Self {
        Self {
            store,
            bucket_id,
            now,
        }
    }

    /// The bucket id.
    pub fn bucket_id(&self) -> BucketId {
        self.bucket_id
    }

    /// Read the current root (see the module docs).
    pub async fn load_root(&self) -> Result<Root> {
        let Some(cid) = self.last_leaf().await? else {
            return Ok(Root {
                cid: None,
                node: DirectoryNode::new_empty(self.bucket_id),
            });
        };
        let not_fs = |reason: String| FsClientError::NotAFileSystemBucket {
            bucket_id: self.bucket_id,
            reason,
        };
        let max = DirectoryNode::max_blob_len() as u64;
        let bytes = self
            .store
            .get_blob(self.bucket_id, cid, BlobLength::AtMost(max))
            .await?;
        if bytes.len() as u64 > max {
            return Err(not_fs(format!(
                "last leaf is larger than a directory node can be ({max} bytes)"
            )));
        }
        check_cid(cid, &bytes)?;
        let node = DirectoryNode::from_blob(&bytes).map_err(|e| match e {
            FileSystemError::UnsupportedVersion(version) => {
                FsClientError::UnsupportedFormatVersion(version)
            }
            other => not_fs(format!("last leaf is not a directory node: {other}")),
        })?;
        node.validate()
            .map_err(|e| not_fs(format!("last leaf is not a valid directory node: {e}")))?;
        if node.bucket_id != self.bucket_id {
            return Err(not_fs(format!(
                "root directory belongs to bucket {}",
                node.bucket_id
            )));
        }
        Ok(Root {
            cid: Some(cid),
            node,
        })
    }

    /// List the entries of the directory at `path`, sorted by name.
    pub async fn list(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let names = parse_path(path)?;
        let root = self.load_root().await?;
        let chain = self.dir_chain(root.node, &names, false).await?;
        Ok(chain
            .into_iter()
            .next_back()
            .map(|dir| dir.children.into_inner())
            .unwrap_or_default())
    }

    /// List the entries of the directory at `path`, sorted by name, without
    /// the subdirectories that contain no file at any depth. Reads each
    /// subdirectory only until it finds a file.
    pub async fn list_non_empty(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let names = parse_path(path)?;
        let base = join(&names);
        let mut entries = Vec::new();
        for entry in self.list(path).await? {
            let child = format!("{}/{}", base.trim_end_matches('/'), entry.name_str());
            if entry.is_file() || self.contains_file(&child, entry.cid).await? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Read the entry and manifest of the file at `path`.
    pub async fn stat(&self, path: &str) -> Result<FileStat> {
        let names = parse_path(path)?;
        let (name, parents) = names
            .split_last()
            .ok_or_else(|| FsClientError::NotAFile(path.to_string()))?;
        let root = self.load_root().await?;
        let chain = self.dir_chain(root.node, parents, false).await?;
        let entry = chain
            .last()
            .and_then(|dir| dir.find_child(name))
            .ok_or_else(|| FsClientError::PathNotFound(path.to_string()))?
            .clone();
        if !entry.is_file() {
            return Err(FsClientError::NotAFile(path.to_string()));
        }
        let manifest = self.read_manifest(path, entry.cid).await?;
        Ok(FileStat { entry, manifest })
    }

    /// Read the file at `path`.
    pub async fn get_file(&self, path: &str) -> Result<FileContent> {
        let stat = self.stat(path).await?;
        let data = if stat.size() == 0 {
            Vec::new()
        } else {
            self.read_blob(path, stat.content_root(), BlobLength::Exact(stat.size()))
                .await?
        };
        Ok(FileContent { stat, data })
    }

    /// Every file under the directory at `path`, at any depth, as
    /// (path relative to `path`, entry). Order is unspecified.
    pub async fn files_under(&self, path: &str) -> Result<Vec<(String, DirectoryEntry)>> {
        let entries = self.entries_under(path).await?;
        Ok(entries
            .into_iter()
            .filter(|(_, e)| !e.is_directory())
            .collect())
    }

    /// Every entry below the directory at `path`, directories and files,
    /// depth-first, each with its path relative to `path`. Reads the root once.
    pub async fn entries_under(&self, path: &str) -> Result<Vec<(String, DirectoryEntry)>> {
        let names = parse_path(path)?;
        let root = self.load_root().await?;
        let start = self
            .dir_chain(root.node, &names, false)
            .await?
            .pop()
            .ok_or_else(|| FsClientError::PathNotFound(path.to_string()))?;

        let mut entries = Vec::new();
        let mut pending = vec![(String::new(), start)];
        while let Some((prefix, dir)) = pending.pop() {
            for entry in dir.children.into_inner() {
                let rel = format!("{prefix}{}", entry.name_str());
                if entry.is_directory() {
                    let child = self.read_dir(&rel, entry.cid).await?;
                    pending.push((format!("{rel}/"), child));
                }
                entries.push((rel, entry));
            }
        }
        Ok(entries)
    }

    /// Write `data` as the file at `path`. Creates missing parent
    /// directories and replaces an existing file. Returns the CID of the new
    /// root directory and the file's stat. Fails with
    /// [`FsClientError::NotAFile`] if `path` is a directory and with
    /// [`FsClientError::NotADirectory`] if a parent is a file.
    pub async fn put_file(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
        user_metadata: Vec<MetadataEntry>,
    ) -> Result<(Cid, FileStat)> {
        let names = parse_path(path)?;
        let (name, parents) = names
            .split_last()
            .ok_or_else(|| FsClientError::NotAFile(path.to_string()))?;
        let manifest = FileManifest::for_content(self.bucket_id, data, content_type, user_metadata)
            .map_err(format_error(path))?;

        let root = self.load_root().await?;
        let mut chain = self.dir_chain(root.node, parents, true).await?;
        let parent = &mut chain[parents.len()];
        if parent.find_child(name).is_some_and(|e| e.is_directory()) {
            return Err(FsClientError::NotAFile(path.to_string()));
        }

        let manifest_bytes = manifest.to_blob();
        let entry = DirectoryEntry::try_new(
            name,
            EntryType::File,
            compute_cid(&manifest_bytes),
            data.len() as u64,
            (self.now)(),
        )
        .map_err(format_error(path))?;
        parent
            .upsert_child(entry.clone())
            .map_err(format_error(path))?;

        let (dirs, root) = self.rewrite(chain, parents, path)?;
        let mut blobs = Vec::new();
        if !data.is_empty() {
            blobs.push(data);
        }
        blobs.push(manifest_bytes.as_slice());
        blobs.extend(dirs.iter().map(Vec::as_slice));
        let root_cid = self.write(&blobs, &root).await?;
        Ok((root_cid, FileStat { entry, manifest }))
    }

    /// Create an empty directory at `path`, and any missing parents. Returns
    /// the CID of the new root directory. Fails with
    /// [`FsClientError::EntryExists`] if `path` exists.
    pub async fn mkdir(&self, path: &str) -> Result<Cid> {
        let names = parse_path(path)?;
        let (name, parents) = names
            .split_last()
            .ok_or_else(|| FsClientError::EntryExists(path.to_string()))?;
        let root = self.load_root().await?;
        let mut chain = self.dir_chain(root.node, parents, true).await?;
        if chain
            .last()
            .and_then(|parent| parent.find_child(name))
            .is_some()
        {
            return Err(FsClientError::EntryExists(path.to_string()));
        }
        chain.push(DirectoryNode::new_empty(self.bucket_id));
        let (dirs, root) = self.rewrite(chain, &names, path)?;
        self.write(&dirs.iter().map(Vec::as_slice).collect::<Vec<_>>(), &root)
            .await
    }

    /// Delete the file or empty directory at `path` (see [`EmptyParents`]).
    /// Returns the CID of the new root directory. Fails with
    /// [`FsClientError::PathNotFound`] if it does not exist,
    /// [`FsClientError::DirectoryNotEmpty`] for a non-empty directory, and
    /// [`FsClientError::InvalidPath`] for `/`.
    pub async fn delete(&self, path: &str, empty_parents: EmptyParents) -> Result<Cid> {
        let names = parse_path(path)?;
        let (name, parents) = names
            .split_last()
            .ok_or_else(|| FsClientError::InvalidPath(path.to_string()))?;
        let root = self.load_root().await?;
        let mut chain = self.dir_chain(root.node, parents, false).await?;
        let entry = chain[parents.len()]
            .remove_child(name)
            .ok_or_else(|| FsClientError::PathNotFound(path.to_string()))?;
        if entry.is_directory() && empty_parents == EmptyParents::Remove {
            return Err(FsClientError::NotAFile(path.to_string()));
        }
        if entry.is_directory() && !self.read_dir(path, entry.cid).await?.children.is_empty() {
            return Err(FsClientError::DirectoryNotEmpty(path.to_string()));
        }

        let mut kept = parents.len();
        if empty_parents == EmptyParents::Remove {
            while kept > 0 && chain[kept].children.is_empty() {
                chain.pop();
                chain[kept - 1].remove_child(parents[kept - 1]);
                kept -= 1;
            }
        }
        let (dirs, root) = self.rewrite(chain, &parents[..kept], path)?;
        self.write(&dirs.iter().map(Vec::as_slice).collect::<Vec<_>>(), &root)
            .await
    }

    /// Move the file or directory at `from` to `to` in one commit, without
    /// copying content. Creates missing parents of `to` and keeps the entry's
    /// `mtime`. Fails with [`FsClientError::PathNotFound`] if `from` does not
    /// exist, [`FsClientError::EntryExists`] if `to` exists, and
    /// [`FsClientError::InvalidPath`] for `/` or a `to` inside `from`.
    /// Returns the CID of the new root directory.
    pub async fn rename(&self, from: &str, to: &str) -> Result<Cid> {
        let from_names = parse_path(from)?;
        let to_names = parse_path(to)?;
        let (Some((from_name, from_parents)), Some((to_name, to_parents))) =
            (from_names.split_last(), to_names.split_last())
        else {
            return Err(FsClientError::InvalidPath("/".to_string()));
        };
        if to_names.starts_with(&from_names) {
            return Err(FsClientError::InvalidPath(to.to_string()));
        }
        let common = from_parents
            .iter()
            .zip(to_parents)
            .take_while(|(from_parent, to_parent)| from_parent == to_parent)
            .count();

        let root = self.load_root().await?;
        let mut chain = self.dir_chain(root.node, from_parents, false).await?;
        let entry = chain[from_parents.len()]
            .remove_child(from_name)
            .ok_or_else(|| FsClientError::PathNotFound(from.to_string()))?;
        let mut dirs = self.rewrite_below(&mut chain, from_parents, common, from)?;
        let mut chain = self.extend_chain(chain, to_parents, true).await?;
        let parent = &mut chain[to_parents.len()];
        if parent.find_child(to_name).is_some() {
            return Err(FsClientError::EntryExists(to.to_string()));
        }
        let moved = DirectoryEntry::try_new(
            to_name,
            entry.entry_type,
            entry.cid,
            entry.size,
            entry.mtime,
        )
        .map_err(format_error(to))?;
        parent.upsert_child(moved).map_err(format_error(to))?;
        let (to_dirs, root) = self.rewrite(chain, to_parents, to)?;
        dirs.extend(to_dirs);
        self.write(&dirs.iter().map(Vec::as_slice).collect::<Vec<_>>(), &root)
            .await
    }

    /// The directories along `names`, starting with `root`. With
    /// `create_missing`, a missing directory is a new empty one; otherwise it
    /// fails with [`FsClientError::PathNotFound`].
    async fn dir_chain(
        &self,
        root: DirectoryNode,
        names: &[&str],
        create_missing: bool,
    ) -> Result<Vec<DirectoryNode>> {
        self.extend_chain(vec![root], names, create_missing).await
    }

    /// Extend `chain`, the directories along the first `chain.len() - 1`
    /// names, to the directories along all of `names` (see
    /// [`Tree::dir_chain`]).
    async fn extend_chain(
        &self,
        mut chain: Vec<DirectoryNode>,
        names: &[&str],
        create_missing: bool,
    ) -> Result<Vec<DirectoryNode>> {
        for depth in chain.len() - 1..names.len() {
            let path = join(&names[..=depth]);
            let next = match chain[depth].find_child(names[depth]) {
                Some(entry) if entry.is_directory() => self.read_dir(&path, entry.cid).await?,
                Some(_) => return Err(FsClientError::NotADirectory(path)),
                None if create_missing => DirectoryNode::new_empty(self.bucket_id),
                None => return Err(FsClientError::PathNotFound(path)),
            };
            chain.push(next);
        }
        Ok(chain)
    }

    /// Encode `chain` (the directories along `names`, starting with the root)
    /// bottom-up. Each directory's new CID goes into its parent's entry.
    /// Returns the encodings of the directories below the root, bottom-up,
    /// and the encoding of the root.
    fn rewrite(
        &self,
        mut chain: Vec<DirectoryNode>,
        names: &[&str],
        path: &str,
    ) -> Result<(Vec<Vec<u8>>, Vec<u8>)> {
        let below_root = self.rewrite_below(&mut chain, names, 0, path)?;
        Ok((below_root, chain[0].to_blob()))
    }

    /// Encode the directories of `chain` (the directories along `names`,
    /// starting with the root) deeper than `depth`, bottom-up, put each new
    /// CID into the parent's entry, and remove them from `chain`. Returns
    /// their encodings, bottom-up.
    fn rewrite_below(
        &self,
        chain: &mut Vec<DirectoryNode>,
        names: &[&str],
        depth: usize,
        path: &str,
    ) -> Result<Vec<Vec<u8>>> {
        debug_assert_eq!(chain.len(), names.len() + 1);
        let mtime = (self.now)();
        let mut encoded = Vec::with_capacity(names.len() - depth);
        for (index, name) in names.iter().enumerate().skip(depth).rev() {
            let bytes = chain[index + 1].to_blob();
            let entry =
                DirectoryEntry::try_new(name, EntryType::Directory, compute_cid(&bytes), 0, mtime)
                    .map_err(format_error(path))?;
            chain[index]
                .upsert_child(entry)
                .map_err(format_error(path))?;
            encoded.push(bytes);
        }
        chain.truncate(depth + 1);
        Ok(encoded)
    }

    /// Upload `blobs`, then `root`, and commit their CIDs in one request, in
    /// order, each once, `root` last. Returns the CID of `root`.
    async fn write(&self, blobs: &[&[u8]], root: &[u8]) -> Result<Cid> {
        let root_cid = compute_cid(root);
        let cids: Vec<Cid> = blobs
            .iter()
            .map(|b| compute_cid(b))
            .chain([root_cid])
            .collect();
        let mut uploaded = HashSet::new();
        for (blob, &expected) in blobs.iter().copied().chain([root]).zip(&cids) {
            if !uploaded.insert(expected) {
                continue;
            }
            let got = self.store.put_blob(self.bucket_id, blob).await?;
            if got != expected {
                return Err(FsClientError::CidMismatch { expected, got });
            }
        }
        self.store
            .commit(self.bucket_id, commit_order(&cids))
            .await?;
        Ok(root_cid)
    }

    /// Data root of the bucket's last MMR leaf, or `None` if the bucket has
    /// no leaves. Fails with [`FsClientError::InvalidMmrProof`] unless the
    /// store's proof proves that leaf against the store's commitment, after
    /// one more read for a commit between the two reads. The provider signs
    /// the commitment; this does not check that signature.
    async fn last_leaf(&self) -> Result<Option<Cid>> {
        match self.read_last_leaf().await {
            Err(FsClientError::InvalidMmrProof(_)) => self.read_last_leaf().await,
            result => result,
        }
    }

    /// One read of [`Tree::last_leaf`]. A commit between the commitment and
    /// the proof read makes the proof one of a larger MMR, which fails with
    /// [`FsClientError::InvalidMmrProof`].
    async fn read_last_leaf(&self) -> Result<Option<Cid>> {
        let Some(commitment) = self.store.commitment(self.bucket_id).await? else {
            return Ok(None);
        };
        let Some(leaf_index) = commitment.leaf_count.checked_sub(1) else {
            return Ok(None);
        };
        let proof = self.store.mmr_proof(self.bucket_id, leaf_index).await?;
        let data_root = proof.leaf.data_root;
        if !verify_mmr_proof(proof, leaf_index, &commitment) {
            return Err(FsClientError::InvalidMmrProof(self.bucket_id));
        }
        Ok(Some(data_root))
    }

    /// Read the blob of the entry at `path` and check its length and that
    /// its CID is `cid`.
    async fn read_blob(&self, path: &str, cid: Cid, length: BlobLength) -> Result<Vec<u8>> {
        let bytes = self.store.get_blob(self.bucket_id, cid, length).await?;
        let len = bytes.len() as u64;
        let length_ok = match length {
            BlobLength::Exact(size) => len == size,
            BlobLength::AtMost(max) => len <= max,
        };
        if !length_ok {
            return Err(FsClientError::InvalidBlob {
                path: path.to_string(),
                reason: format!("{len} bytes, expected {length:?}"),
            });
        }
        check_cid(cid, &bytes)?;
        Ok(bytes)
    }

    async fn read_dir(&self, path: &str, cid: Cid) -> Result<DirectoryNode> {
        let bytes = self
            .read_blob(
                path,
                cid,
                BlobLength::AtMost(DirectoryNode::max_blob_len() as u64),
            )
            .await?;
        let node = DirectoryNode::from_blob(&bytes).map_err(invalid_blob(path))?;
        node.validate().map_err(invalid_blob(path))?;
        Ok(node)
    }

    /// Whether the directory `cid` at `path` contains a file at any depth.
    async fn contains_file(&self, path: &str, cid: Cid) -> Result<bool> {
        let mut pending = vec![(path.to_string(), cid)];
        while let Some((path, cid)) = pending.pop() {
            let dir = self.read_dir(&path, cid).await?;
            if dir.children.iter().any(DirectoryEntry::is_file) {
                return Ok(true);
            }
            pending.extend(
                dir.children
                    .into_inner()
                    .into_iter()
                    .map(|entry| (format!("{path}/{}", entry.name_str()), entry.cid)),
            );
        }
        Ok(false)
    }

    async fn read_manifest(&self, path: &str, cid: Cid) -> Result<FileManifest> {
        let bytes = self
            .read_blob(
                path,
                cid,
                BlobLength::AtMost(FileManifest::max_blob_len() as u64),
            )
            .await?;
        let manifest = FileManifest::from_blob(&bytes).map_err(invalid_blob(path))?;
        manifest.validate().map_err(invalid_blob(path))?;
        Ok(manifest)
    }
}

/// Fail with [`FsClientError::CidMismatch`] unless `bytes` has CID `cid`.
fn check_cid(cid: Cid, bytes: &[u8]) -> Result<()> {
    let got = compute_cid(bytes);
    if got != cid {
        return Err(FsClientError::CidMismatch { expected: cid, got });
    }
    Ok(())
}

/// `cids` in commit order: first occurrence order, each once, and the last
/// CID (the root) last.
fn commit_order(cids: &[Cid]) -> Vec<Cid> {
    let Some((root, rest)) = cids.split_last() else {
        return Vec::new();
    };
    let mut seen = HashSet::from([*root]);
    let mut order: Vec<Cid> = rest.iter().copied().filter(|c| seen.insert(*c)).collect();
    order.push(*root);
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{tree, MemoryStore, BUCKET};

    /// A commit by another writer between the commitment and the proof
    /// gives a proof of a larger MMR, which does not prove the commitment's
    /// last leaf. Two such commits in a row fail the root read.
    #[tokio::test]
    async fn root_read_rejects_a_proof_of_a_later_mmr_twice() {
        let store = MemoryStore::default();
        tree(&store).mkdir("/a").await.unwrap();
        store
            .commits_after_commitment
            .lock()
            .unwrap()
            .extend([b"other writer".to_vec(), b"other writer again".to_vec()]);
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::InvalidMmrProof(BUCKET))
        ));
    }

    /// A root of a newer format version is reported as such, not as data
    /// that is not a file system.
    #[tokio::test]
    async fn a_newer_format_version_is_reported() {
        let store = MemoryStore::default();
        let mut newer = DirectoryNode::new_empty(BUCKET).to_blob();
        newer[4] = crate::format::FORMAT_VERSION + 1;
        store.raw_commit(&newer);
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::UnsupportedFormatVersion(v)) if v == crate::format::FORMAT_VERSION + 1
        ));
    }

    #[tokio::test]
    async fn put_rejects_duplicate_metadata_keys() {
        let store = MemoryStore::default();
        let entry = MetadataEntry::try_new(b"k", b"v").unwrap();
        assert!(matches!(
            tree(&store)
                .put_file("/f", b"x", "", vec![entry.clone(), entry])
                .await,
            Err(FsClientError::Format {
                source: FileSystemError::DuplicateMetadataKey,
                ..
            })
        ));
    }

    /// One commit by another writer between the commitment and the proof
    /// read costs one more read, which sees that writer's root.
    #[tokio::test]
    async fn root_read_retries_after_a_commit_between_the_two_reads() {
        let store = MemoryStore::default();
        tree(&store).mkdir("/a").await.unwrap();
        let other_root = DirectoryNode::new_empty(BUCKET).to_blob();
        store
            .commits_after_commitment
            .lock()
            .unwrap()
            .push_back(other_root.clone());
        let root = tree(&store).load_root().await.unwrap();
        assert_eq!(root.cid, Some(compute_cid(&other_root)));
    }

    /// Every directory CID reachable from the root, the root included.
    async fn directory_cids(t: &Tree<&MemoryStore>) -> HashSet<Cid> {
        let root = t.load_root().await.unwrap();
        let mut found = HashSet::from([root.cid.unwrap()]);
        let mut pending = vec![root.node];
        while let Some(dir) = pending.pop() {
            for entry in dir.children.into_inner() {
                if entry.is_directory() {
                    found.insert(entry.cid);
                    pending.push(t.read_dir("/", entry.cid).await.unwrap());
                }
            }
        }
        found
    }

    /// A rename moves the entry in one commit without uploading the content
    /// again, and commits only directories the new tree references.
    #[tokio::test]
    async fn rename_moves_an_entry_in_one_commit() {
        let store = MemoryStore::default();
        let t = tree(&store);
        let (_, put) = t
            .put_file("/a/x/f.txt", b"hello", "text/plain", vec![])
            .await
            .unwrap();
        t.mkdir("/a/y").await.unwrap();
        let commits_before = store.commits().len();

        let root = t.rename("/a/x/f.txt", "/a/y/z/g.txt").await.unwrap();

        let commits = store.commits();
        assert_eq!(commits.len(), commits_before + 1);
        let rename_commit = commits.last().unwrap();
        assert_eq!(rename_commit.last(), Some(&root));
        assert!(!rename_commit.contains(&compute_cid(b"hello")));
        let reachable = directory_cids(&t).await;
        assert!(rename_commit.iter().all(|cid| reachable.contains(cid)));

        let moved = t.stat("/a/y/z/g.txt").await.unwrap();
        assert_eq!(moved.entry.cid, put.entry.cid);
        assert_eq!(moved.mtime(), put.mtime());
        assert_eq!(t.get_file("/a/y/z/g.txt").await.unwrap().data, b"hello");
        assert!(matches!(
            t.stat("/a/x/f.txt").await,
            Err(FsClientError::PathNotFound(_))
        ));
        assert!(t.list("/a/x").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rename_moves_a_directory_with_its_subtree() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/d/e/f", b"deep", "", vec![]).await.unwrap();
        t.rename("/d", "/top/d2").await.unwrap();
        assert_eq!(t.get_file("/top/d2/e/f").await.unwrap().data, b"deep");
        assert_eq!(names(&t.list("/").await.unwrap()), ["top"]);

        t.rename("/top/d2/e/f", "/top/d2/e/f2").await.unwrap();
        assert_eq!(names(&t.list("/top/d2/e").await.unwrap()), ["f2"]);
    }

    #[tokio::test]
    async fn rename_rejects_missing_existing_and_nested_paths() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/a/f", b"x", "", vec![]).await.unwrap();
        t.put_file("/b", b"y", "", vec![]).await.unwrap();
        assert!(matches!(
            t.rename("/missing", "/c").await,
            Err(FsClientError::PathNotFound(_))
        ));
        assert!(matches!(
            t.rename("/a/f", "/b").await,
            Err(FsClientError::EntryExists(_))
        ));
        assert!(matches!(
            t.rename("/a", "/a/inner").await,
            Err(FsClientError::InvalidPath(_))
        ));
        assert!(matches!(
            t.rename("/", "/c").await,
            Err(FsClientError::InvalidPath(_))
        ));
    }

    /// The root CID a write returns is the bucket's new last leaf.
    #[tokio::test]
    async fn writes_return_the_new_root() {
        let store = MemoryStore::default();
        let t = tree(&store);
        let (put, _) = t.put_file("/f", b"x", "", vec![]).await.unwrap();
        assert_eq!(t.load_root().await.unwrap().cid, Some(put));
        let mkdir = t.mkdir("/d").await.unwrap();
        assert_eq!(t.load_root().await.unwrap().cid, Some(mkdir));
        let rename = t.rename("/d", "/e").await.unwrap();
        assert_eq!(t.load_root().await.unwrap().cid, Some(rename));
        let delete = t.delete("/e", EmptyParents::Keep).await.unwrap();
        assert_eq!(t.load_root().await.unwrap().cid, Some(delete));
    }

    fn names(entries: &[DirectoryEntry]) -> Vec<String> {
        entries.iter().map(|e| e.name_str()).collect()
    }

    #[test]
    fn path_rules() {
        assert_eq!(parse_path("/").unwrap(), Vec::<&str>::new());
        assert_eq!(parse_path("/a").unwrap(), vec!["a"]);
        assert_eq!(parse_path("/a/b.txt").unwrap(), vec!["a", "b.txt"]);
        let long = format!("/{}", "x".repeat(256));
        assert!(parse_path(&long).is_ok());
        for bad in [
            "",
            "a",
            "//",
            "/a/",
            "/a//b",
            "/./a",
            "/a/..",
            &format!("/{}", "x".repeat(257)),
        ] {
            assert!(
                matches!(parse_path(bad), Err(FsClientError::InvalidPath(_))),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn commit_order_dedupes_and_puts_root_last() {
        let [x, y, root] = [&b"x"[..], b"y", b"root"].map(compute_cid);
        assert_eq!(commit_order(&[x, y, x, root, y, root]), vec![x, y, root]);
        assert!(commit_order(&[]).is_empty());
    }

    #[tokio::test]
    async fn empty_bucket_has_empty_root_and_reads_do_not_commit() {
        let store = MemoryStore::default();
        let root = tree(&store).load_root().await.unwrap();
        assert_eq!(root.cid, None);
        assert_eq!(root.node, DirectoryNode::new_empty(BUCKET));
        assert!(tree(&store).list("/").await.unwrap().is_empty());
        assert!(matches!(
            tree(&store).stat("/missing").await,
            Err(FsClientError::PathNotFound(_))
        ));
        assert!(store.commits().is_empty());
    }

    #[tokio::test]
    async fn list_non_empty_drops_directories_without_files() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/f", b"x", DEFAULT, vec![]).await.unwrap();
        t.put_file("/deep/a/b/f", b"x", DEFAULT, vec![])
            .await
            .unwrap();
        t.mkdir("/deep/empty").await.unwrap();
        t.mkdir("/nested/empty").await.unwrap();

        assert_eq!(names(&t.list_non_empty("/").await.unwrap()), ["deep", "f"]);
        assert_eq!(names(&t.list_non_empty("/deep").await.unwrap()), ["a"]);
        assert!(t.list_non_empty("/nested").await.unwrap().is_empty());
        assert!(matches!(
            t.list_non_empty("/missing").await,
            Err(FsClientError::PathNotFound(_))
        ));
    }

    #[tokio::test]
    async fn put_file_commits_content_manifest_directories_then_root() {
        let store = MemoryStore::default();
        let (new_root, stat) = tree(&store)
            .put_file("/a/b/f.txt", b"hello", "text/plain", vec![])
            .await
            .unwrap();

        let commits = store.commits();
        assert_eq!(commits.len(), 1);
        let order = &commits[0];
        assert_eq!(order.len(), 5);
        assert_eq!(order[0], compute_cid(b"hello"));
        assert_eq!(order[1], stat.entry.cid);

        // Directories bottom-up: b, a, root.
        let blobs = store.blobs.lock().unwrap().clone();
        let b = DirectoryNode::from_blob(&blobs[&order[2]]).unwrap();
        assert_eq!(names(&b.children), ["f.txt"]);
        let a = DirectoryNode::from_blob(&blobs[&order[3]]).unwrap();
        assert_eq!(a.find_child("b").unwrap().cid, order[2]);
        let root = DirectoryNode::from_blob(&blobs[&order[4]]).unwrap();
        assert_eq!(root.find_child("a").unwrap().cid, order[3]);

        assert_eq!(new_root, order[4]);
        let root = tree(&store).load_root().await.unwrap();
        assert_eq!(root.cid, Some(order[4]));

        let file = tree(&store).get_file("/a/b/f.txt").await.unwrap();
        assert_eq!(file.data, b"hello");
        assert_eq!(file.stat.content_type(), "text/plain");
        assert_eq!(file.stat.size(), 5);
        assert_eq!(file.stat.entry.size, 5);
    }

    #[tokio::test]
    async fn empty_file_has_a_manifest_and_no_content_blob() {
        let store = MemoryStore::default();
        let (_, stat) = tree(&store)
            .put_file("/empty", b"", DEFAULT, vec![])
            .await
            .unwrap();
        assert!(stat.manifest.chunks.is_empty());

        // Manifest, root directory, then no content blob.
        let order = &store.commits()[0];
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], stat.entry.cid);

        // Reading it makes no store request: the store has no content blob.
        let file = tree(&store).get_file("/empty").await.unwrap();
        assert!(file.data.is_empty());
        assert_eq!(file.stat.size(), 0);
    }

    #[tokio::test]
    async fn write_rewrites_only_the_changed_path() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/a/x", b"1", DEFAULT, vec![]).await.unwrap();
        t.put_file("/b/y", b"2", DEFAULT, vec![]).await.unwrap();
        let before = t.list("/").await.unwrap();
        let b_before = before.iter().find(|e| e.name_str() == "b").unwrap().cid;
        let a_before = before.iter().find(|e| e.name_str() == "a").unwrap().cid;

        t.put_file("/a/z", b"3", DEFAULT, vec![]).await.unwrap();
        let after = t.list("/").await.unwrap();
        let b_after = after.iter().find(|e| e.name_str() == "b").unwrap().cid;
        let a_after = after.iter().find(|e| e.name_str() == "a").unwrap().cid;
        assert_eq!(b_before, b_after, "untouched subtree keeps its CID");
        assert_ne!(a_before, a_after);
        let last = store.commits().pop().unwrap();
        assert!(!last.contains(&b_after));
        assert_eq!(names(&t.list("/a").await.unwrap()), ["x", "z"]);
    }

    const DEFAULT: &str = crate::format::DEFAULT_MIME_TYPE;

    #[tokio::test]
    async fn put_file_replaces_and_rejects_directories() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/d/f", b"old", DEFAULT, vec![]).await.unwrap();
        t.put_file("/d/f", b"new!", DEFAULT, vec![]).await.unwrap();
        assert_eq!(t.get_file("/d/f").await.unwrap().data, b"new!");
        assert_eq!(t.list("/d").await.unwrap().len(), 1);

        assert!(matches!(
            t.put_file("/d", b"x", DEFAULT, vec![]).await,
            Err(FsClientError::NotAFile(_))
        ));
        assert!(matches!(
            t.put_file("/d/f/g", b"x", DEFAULT, vec![]).await,
            Err(FsClientError::NotADirectory(p)) if p == "/d/f"
        ));
        assert!(matches!(
            t.put_file("/", b"x", DEFAULT, vec![]).await,
            Err(FsClientError::NotAFile(_))
        ));
    }

    #[tokio::test]
    async fn mkdir_creates_parents_and_rejects_existing() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.mkdir("/a/b/c").await.unwrap();
        assert_eq!(names(&t.list("/a/b").await.unwrap()), ["c"]);
        assert!(t.list("/a/b/c").await.unwrap().is_empty());
        assert!(matches!(
            t.mkdir("/a/b").await,
            Err(FsClientError::EntryExists(_))
        ));
        assert!(matches!(
            t.mkdir("/").await,
            Err(FsClientError::EntryExists(_))
        ));

        // The new empty directory is committed before its parents.
        let order = store.commits().pop().unwrap();
        assert_eq!(
            order[0],
            DirectoryNode::new_empty(BUCKET).compute_cid(),
            "new directory first"
        );
        assert_eq!(order.len(), 4);
    }

    #[tokio::test]
    async fn delete_file_and_empty_directory() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/d/f", b"x", DEFAULT, vec![]).await.unwrap();
        assert!(matches!(
            t.delete("/d", EmptyParents::Keep).await,
            Err(FsClientError::DirectoryNotEmpty(_))
        ));
        t.delete("/d/f", EmptyParents::Keep).await.unwrap();
        assert!(t.list("/d").await.unwrap().is_empty());
        t.delete("/d", EmptyParents::Keep).await.unwrap();
        assert!(t.list("/").await.unwrap().is_empty());

        assert!(matches!(
            t.delete("/d", EmptyParents::Keep).await,
            Err(FsClientError::PathNotFound(_))
        ));
        assert!(matches!(
            t.delete("/", EmptyParents::Keep).await,
            Err(FsClientError::InvalidPath(_))
        ));
        // After the last delete the root is an empty directory, still committed.
        assert_eq!(
            t.load_root().await.unwrap().cid,
            Some(DirectoryNode::new_empty(BUCKET).compute_cid())
        );
    }

    #[tokio::test]
    async fn delete_can_remove_empty_parents() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/a/b/c/f", b"x", DEFAULT, vec![]).await.unwrap();
        t.put_file("/a/g", b"y", DEFAULT, vec![]).await.unwrap();
        t.delete("/a/b/c/f", EmptyParents::Remove).await.unwrap();
        assert_eq!(names(&t.list("/a").await.unwrap()), ["g"]);
        t.delete("/a/g", EmptyParents::Remove).await.unwrap();
        assert!(t.list("/").await.unwrap().is_empty());

        // Remove deletes only files.
        t.mkdir("/empty").await.unwrap();
        assert!(matches!(
            t.delete("/empty", EmptyParents::Remove).await,
            Err(FsClientError::NotAFile(_))
        ));
    }

    #[tokio::test]
    async fn files_under_walks_all_depths() {
        let store = MemoryStore::default();
        let t = tree(&store);
        for path in ["/top", "/a/x", "/a/b/y", "/c/z"] {
            t.put_file(path, b"1", DEFAULT, vec![]).await.unwrap();
        }
        let mut all: Vec<String> = t
            .files_under("/")
            .await
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        all.sort();
        assert_eq!(all, ["a/b/y", "a/x", "c/z", "top"]);
        let mut under_a: Vec<String> = t
            .files_under("/a")
            .await
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        under_a.sort();
        assert_eq!(under_a, ["b/y", "x"]);
    }

    #[tokio::test]
    async fn multi_chunk_directory_round_trips() {
        let store = MemoryStore::default();
        let t = tree(&store);
        // 1000 names of 250 bytes: the root directory is over 256 KiB.
        let mut root = DirectoryNode::new_empty(BUCKET);
        for i in 0..1000 {
            let name = format!("{i:0>250}");
            root.upsert_child(
                DirectoryEntry::try_new(&name, EntryType::File, compute_cid(b"m"), 1, 0).unwrap(),
            )
            .unwrap();
        }
        let bytes = root.to_blob();
        assert!(bytes.len() > crate::format::CHUNK_SIZE);
        store.raw_commit(&bytes);
        let loaded = t.load_root().await.unwrap();
        assert_eq!(loaded.node, root);
        assert_eq!(loaded.cid, Some(compute_cid(&bytes)));
    }

    #[tokio::test]
    async fn last_leaf_that_is_not_a_directory_is_rejected() {
        let store = MemoryStore::default();
        store.raw_commit(b"raw layer 0 data");
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::NotAFileSystemBucket {
                bucket_id: BUCKET,
                ..
            })
        ));

        let store = MemoryStore::default();
        store.raw_commit(&DirectoryNode::new_empty(BUCKET + 1).to_blob());
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::NotAFileSystemBucket { .. })
        ));
    }

    /// The client reports the format rule that rejected a write, as the
    /// primitives error, with the path; nothing is uploaded.
    #[tokio::test]
    async fn a_write_that_breaks_a_format_rule_reports_the_rule() {
        let store = MemoryStore::default();
        let content_type = "x".repeat(129);
        let err = tree(&store)
            .put_file("/a.txt", b"x", &content_type, vec![])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            FsClientError::Format { ref path, source: FileSystemError::MetadataTooLarge }
                if path == "/a.txt"
        ));
        assert!(store.commits().is_empty());
    }

    #[tokio::test]
    async fn reads_reject_bytes_that_do_not_match_the_cid() {
        let store = MemoryStore::default();
        let t = tree(&store);
        t.put_file("/f", b"real", DEFAULT, vec![]).await.unwrap();
        *store.tampered.lock().unwrap() = Some((compute_cid(b"real"), b"fake".to_vec()));
        assert!(matches!(
            t.get_file("/f").await,
            Err(FsClientError::CidMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn user_metadata_is_stored_in_the_manifest() {
        let store = MemoryStore::default();
        let t = tree(&store);
        let metadata = vec![MetadataEntry::try_new(b"color", b"blue").unwrap()];
        t.put_file("/f", b"x", "image/png", metadata.clone())
            .await
            .unwrap();
        let stat = t.stat("/f").await.unwrap();
        assert_eq!(stat.manifest.user_metadata.to_vec(), metadata);
        assert_eq!(stat.content_type(), "image/png");
        assert_eq!(stat.content_root(), compute_cid(b"x"));
    }

    #[tokio::test]
    async fn empty_content_type_is_the_default() {
        let store = MemoryStore::default();
        let t = tree(&store);
        let (_, stat) = t.put_file("/f", b"x", "", vec![]).await.unwrap();
        assert_eq!(stat.content_type(), DEFAULT);
        assert_eq!(t.stat("/f").await.unwrap().content_type(), DEFAULT);
    }

    /// Commit a root directory with one file entry `/f` whose manifest blob
    /// is `manifest`.
    fn commit_file_with_manifest(store: &MemoryStore, manifest: &[u8]) {
        let cid = compute_cid(manifest);
        store.blobs.lock().unwrap().insert(cid, manifest.to_vec());
        let mut root = DirectoryNode::new_empty(BUCKET);
        root.upsert_child(DirectoryEntry::try_new("f", EntryType::File, cid, 1, 0).unwrap())
            .unwrap();
        store.raw_commit(&root.to_blob());
    }

    #[tokio::test]
    async fn metadata_reads_reject_blobs_over_the_max_encoded_size() {
        let store = MemoryStore::default();
        store.raw_commit(&vec![0u8; DirectoryNode::max_blob_len() + 1]);
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::NotAFileSystemBucket {
                bucket_id: BUCKET,
                ..
            })
        ));

        let store = MemoryStore::default();
        commit_file_with_manifest(&store, &vec![0u8; FileManifest::max_blob_len() + 1]);
        assert!(matches!(
            tree(&store).stat("/f").await,
            Err(FsClientError::InvalidBlob { path, .. }) if path == "/f"
        ));
    }

    #[tokio::test]
    async fn stat_rejects_a_manifest_whose_size_disagrees_with_its_chunks() {
        let store = MemoryStore::default();
        let mut manifest = FileManifest::for_content(BUCKET, b"x", DEFAULT, vec![]).unwrap();
        manifest.total_size = 1 << 20;
        commit_file_with_manifest(&store, &manifest.to_blob());
        assert!(matches!(
            tree(&store).stat("/f").await,
            Err(FsClientError::InvalidBlob { path, .. }) if path == "/f"
        ));
    }
}
