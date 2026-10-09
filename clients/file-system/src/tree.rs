// SPDX-License-Identifier: Apache-2.0

//! The file tree of a drive, stored in its Layer 0 bucket.
//!
//! This module is the only code that knows the on-bucket format; the file
//! system client and the S3 client both use it.
//!
//! # Format
//!
//! Directories are SCALE-encoded [`DirectoryNode`]s and files are a content
//! blob plus a SCALE-encoded [`FileManifest`]. Every blob is addressed by its
//! CID, the Layer 0 data root ([`compute_cid`]).
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
//! - **Reads are unauthenticated.** Anyone who knows a CID can read the blob
//!   (#383, #396). Confidential data needs client-side encryption.

use crate::{FsClientError, Result};
use codec::MaxEncodedLen;
use file_system_primitives::{
    compute_cid, Cid, DirectoryEntry, DirectoryNode, EntryType, FileManifest, FileSystemError,
    MetadataEntry,
};
use std::collections::HashSet;
use storage_client::{ChunkingStrategy, StorageUserClient};
use storage_primitives::BucketId;

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

/// The Layer 0 operations the tree uses.
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    /// Upload `data` to the bucket (chunks and padded Merkle tree nodes) and
    /// return its data root.
    async fn put_blob(&self, bucket_id: BucketId, data: &[u8]) -> Result<Cid>;

    /// Read the blob with data root `cid`.
    async fn get_blob(&self, cid: Cid, length: BlobLength) -> Result<Vec<u8>>;

    /// Append `data_roots` to the bucket's MMR, in order.
    async fn commit(&self, bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()>;

    /// Data root of the bucket's last MMR leaf, or `None` if the bucket has
    /// no leaves.
    async fn last_leaf(&self, bucket_id: BucketId) -> Result<Option<Cid>>;
}

#[async_trait::async_trait]
impl<T: BlobStore + ?Sized> BlobStore for &T {
    async fn put_blob(&self, bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
        (**self).put_blob(bucket_id, data).await
    }

    async fn get_blob(&self, cid: Cid, length: BlobLength) -> Result<Vec<u8>> {
        (**self).get_blob(cid, length).await
    }

    async fn commit(&self, bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
        (**self).commit(bucket_id, data_roots).await
    }

    async fn last_leaf(&self, bucket_id: BucketId) -> Result<Option<Cid>> {
        (**self).last_leaf(bucket_id).await
    }
}

/// Layer 0 over HTTP. The client must not have encryption enabled: the
/// stored bytes would then not match the CIDs the tree computes.
#[async_trait::async_trait]
impl BlobStore for StorageUserClient {
    async fn put_blob(&self, bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
        self.upload(bucket_id, data, ChunkingStrategy::default())
            .await
            .map_err(storage_error)
    }

    /// With [`BlobLength::AtMost`], the read requests one byte more than
    /// the maximum. A 64-byte result has the same CID as an internal Merkle
    /// node over two hashes, so the read then also checks with `GET /node`
    /// that `cid` is a leaf. A provider that lies in both responses is not
    /// detected; only an exact size rules that out.
    async fn get_blob(&self, cid: Cid, length: BlobLength) -> Result<Vec<u8>> {
        let max = match length {
            BlobLength::Exact(size) => size,
            BlobLength::AtMost(max) => max.saturating_add(1),
        };
        let data = self.download(&cid, 0, max).await.map_err(storage_error)?;
        if matches!(length, BlobLength::AtMost(_)) && data.len() == 64 {
            let (_, children) = self.read_node(&cid).await.map_err(storage_error)?;
            if children.is_some() {
                return Err(FsClientError::Serialization(format!(
                    "{cid:?} is an internal Merkle node, not a blob"
                )));
            }
        }
        Ok(data)
    }

    async fn commit(&self, bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
        StorageUserClient::commit(self, bucket_id, data_roots)
            .await
            .map(|_| ())
            .map_err(storage_error)
    }

    /// Reads the commitment, then the MMR proof of leaf `leaf_count - 1`, and
    /// checks that the proof is for the last leaf of the commitment's MMR
    /// ([`verify_last_mmr_leaf`]). The provider signs the commitment; this
    /// does not check that signature.
    async fn last_leaf(&self, bucket_id: BucketId) -> Result<Option<Cid>> {
        let Some(commitment) = self
            .get_commitment_if_exists(bucket_id)
            .await
            .map_err(storage_error)?
        else {
            return Ok(None);
        };
        if commitment.leaf_count == 0 {
            return Ok(None);
        }
        let proof = self
            .get_mmr_proof(bucket_id, commitment.leaf_count - 1)
            .await
            .map_err(storage_error)?;
        if !verify_last_mmr_leaf(&proof, &commitment.mmr_root, commitment.leaf_count) {
            return Err(FsClientError::InvalidMmrProof(bucket_id));
        }
        Ok(Some(proof.leaf.data_root))
    }
}

/// Check that `proof` proves the last leaf of an MMR with `leaf_count`
/// leaves and root `mmr_root`.
///
/// [`storage_primitives::verify_mmr_proof`] accepts a proof of any leaf. The
/// last leaf is the rightmost leaf of the lowest peak, whose height is the
/// number of trailing zero bits of `leaf_count`: the proof has that many
/// siblings, every step goes up from the right, the result is the last peak,
/// and there is one peak per set bit of `leaf_count`.
pub fn verify_last_mmr_leaf(
    proof: &storage_primitives::MmrProof,
    mmr_root: &Cid,
    leaf_count: u64,
) -> bool {
    if leaf_count == 0 || !storage_primitives::verify_mmr_proof(proof, mmr_root) {
        return false;
    }
    let height = leaf_count.trailing_zeros() as usize;
    let path = &proof.leaf_proof;
    if proof.peaks.len() != leaf_count.count_ones() as usize
        || path.siblings.len() != height
        || path.path.len() != height
        || !path.path.iter().all(|right| *right)
    {
        return false;
    }
    let leaf_hash = storage_primitives::blake2_256(&codec::Encode::encode(&proof.leaf));
    let peak = path.siblings.iter().fold(leaf_hash, |current, sibling| {
        storage_primitives::hash_children(*sibling, current)
    });
    proof.peaks.last() == Some(&peak)
}

fn storage_error(e: storage_client::ClientError) -> FsClientError {
    FsClientError::StorageClient(e.to_string())
}

/// Map a format error to the client error for `path`.
fn format_error(path: &str, e: FileSystemError) -> FsClientError {
    match e {
        FileSystemError::InvalidEntryName => FsClientError::InvalidPath(path.to_string()),
        FileSystemError::BoundsExceeded => FsClientError::BoundedOverflow,
        other => FsClientError::Serialization(format!("{path}: {other}")),
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
/// [`file_system_primitives::validate_entry_name`].
pub fn parse_path(path: &str) -> Result<Vec<&str>> {
    let invalid = || FsClientError::InvalidPath(path.to_string());
    let rest = path.strip_prefix('/').ok_or_else(invalid)?;
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    rest.split('/')
        .map(|name| {
            file_system_primitives::validate_entry_name(name.as_bytes())
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
        let Some(cid) = self.store.last_leaf(self.bucket_id).await? else {
            return Ok(Root {
                cid: None,
                node: DirectoryNode::new_empty(self.bucket_id),
            });
        };
        let not_fs = |reason: String| FsClientError::NotAFileSystemBucket {
            bucket_id: self.bucket_id,
            reason,
        };
        let max = max_len::<DirectoryNode>();
        let bytes = self.store.get_blob(cid, BlobLength::AtMost(max)).await?;
        if bytes.len() as u64 > max {
            return Err(not_fs(format!(
                "last leaf is larger than a directory node can be ({max} bytes)"
            )));
        }
        check_cid(cid, &bytes)?;
        let node = DirectoryNode::from_scale_bytes(&bytes)
            .map_err(|e| not_fs(format!("last leaf is not a directory node: {e}")))?;
        node.validate()
            .map_err(|e| not_fs(format!("last leaf is not a valid directory node: {e}")))?;
        if node.drive_id != self.bucket_id {
            return Err(not_fs(format!(
                "root directory belongs to drive {}",
                node.drive_id
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
        let data = self
            .read_blob(stat.content_root(), BlobLength::Exact(stat.size()))
            .await?;
        Ok(FileContent { stat, data })
    }

    /// Every file under the directory at `path`, at any depth, as
    /// (path relative to `path`, entry). Order is unspecified.
    pub async fn files_under(&self, path: &str) -> Result<Vec<(String, DirectoryEntry)>> {
        let names = parse_path(path)?;
        let root = self.load_root().await?;
        let start = self
            .dir_chain(root.node, &names, false)
            .await?
            .pop()
            .ok_or_else(|| FsClientError::PathNotFound(path.to_string()))?;

        let mut files = Vec::new();
        let mut pending = vec![(String::new(), start)];
        while let Some((prefix, dir)) = pending.pop() {
            for entry in dir.children.into_inner() {
                let rel = format!("{prefix}{}", entry.name_str());
                if entry.is_directory() {
                    let child = self.read_dir(&rel, entry.cid).await?;
                    pending.push((format!("{rel}/"), child));
                } else {
                    files.push((rel, entry));
                }
            }
        }
        Ok(files)
    }

    /// Write `data` as the file at `path`. Creates missing parent
    /// directories and replaces an existing file. Fails with
    /// [`FsClientError::NotAFile`] if `path` is a directory and with
    /// [`FsClientError::NotADirectory`] if a parent is a file.
    pub async fn put_file(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
        user_metadata: Vec<MetadataEntry>,
    ) -> Result<FileStat> {
        let names = parse_path(path)?;
        let (name, parents) = names
            .split_last()
            .ok_or_else(|| FsClientError::NotAFile(path.to_string()))?;
        let manifest = FileManifest::for_content(self.bucket_id, data, content_type, user_metadata)
            .map_err(|e| format_error(path, e))?;

        let root = self.load_root().await?;
        let mut chain = self.dir_chain(root.node, parents, true).await?;
        let parent = &mut chain[parents.len()];
        if parent.find_child(name).is_some_and(|e| e.is_directory()) {
            return Err(FsClientError::NotAFile(path.to_string()));
        }

        let manifest_bytes = manifest.to_scale_bytes();
        let entry = DirectoryEntry::try_new(
            name,
            EntryType::File,
            compute_cid(&manifest_bytes),
            data.len() as u64,
            (self.now)(),
        )
        .map_err(|e| format_error(path, e))?;
        parent
            .upsert_child(entry.clone())
            .map_err(|e| format_error(path, e))?;

        let dirs = self.rewrite(chain, parents, path)?;
        let mut blobs = vec![data, manifest_bytes.as_slice()];
        blobs.extend(dirs.iter().map(Vec::as_slice));
        self.write(&blobs).await?;
        Ok(FileStat { entry, manifest })
    }

    /// Create an empty directory at `path`, and any missing parents. Fails
    /// with [`FsClientError::EntryExists`] if `path` exists.
    pub async fn mkdir(&self, path: &str) -> Result<()> {
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
        let dirs = self.rewrite(chain, &names, path)?;
        self.write(&dirs.iter().map(Vec::as_slice).collect::<Vec<_>>())
            .await
    }

    /// Delete the file or empty directory at `path` (see [`EmptyParents`]).
    /// Fails with [`FsClientError::PathNotFound`] if it does not exist,
    /// [`FsClientError::DirectoryNotEmpty`] for a non-empty directory, and
    /// [`FsClientError::InvalidPath`] for `/`.
    pub async fn delete(&self, path: &str, empty_parents: EmptyParents) -> Result<()> {
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
        let dirs = self.rewrite(chain, &parents[..kept], path)?;
        self.write(&dirs.iter().map(Vec::as_slice).collect::<Vec<_>>())
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
        let mut chain = vec![root];
        for depth in 0..names.len() {
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
    /// Returns the encodings, root last.
    fn rewrite(
        &self,
        mut chain: Vec<DirectoryNode>,
        names: &[&str],
        path: &str,
    ) -> Result<Vec<Vec<u8>>> {
        debug_assert_eq!(chain.len(), names.len() + 1);
        let mtime = (self.now)();
        let mut blobs = Vec::with_capacity(chain.len());
        while let Some(dir) = chain.pop() {
            let bytes = dir.to_scale_bytes();
            let depth = chain.len();
            if let Some(parent) = chain.last_mut() {
                let entry = DirectoryEntry::try_new(
                    names[depth - 1],
                    EntryType::Directory,
                    compute_cid(&bytes),
                    0,
                    mtime,
                )
                .map_err(|e| format_error(path, e))?;
                parent
                    .upsert_child(entry)
                    .map_err(|e| format_error(path, e))?;
            }
            blobs.push(bytes);
        }
        Ok(blobs)
    }

    /// Upload `blobs` and commit their CIDs in one request, in order, each
    /// once. The last blob is the new root and is committed last.
    async fn write(&self, blobs: &[&[u8]]) -> Result<()> {
        let cids: Vec<Cid> = blobs.iter().map(|b| compute_cid(b)).collect();
        let mut uploaded = HashSet::new();
        for (blob, &expected) in blobs.iter().zip(&cids) {
            if !uploaded.insert(expected) {
                continue;
            }
            let got = self.store.put_blob(self.bucket_id, blob).await?;
            if got != expected {
                return Err(FsClientError::CidMismatch { expected, got });
            }
        }
        self.store.commit(self.bucket_id, commit_order(&cids)).await
    }

    /// Read a blob and check its length and that its CID is `cid`.
    async fn read_blob(&self, cid: Cid, length: BlobLength) -> Result<Vec<u8>> {
        let bytes = self.store.get_blob(cid, length).await?;
        let len = bytes.len() as u64;
        let length_ok = match length {
            BlobLength::Exact(size) => len == size,
            BlobLength::AtMost(max) => len <= max,
        };
        if !length_ok {
            return Err(FsClientError::Serialization(format!(
                "{cid:?}: {len} bytes, expected {length:?}"
            )));
        }
        check_cid(cid, &bytes)?;
        Ok(bytes)
    }

    async fn read_dir(&self, path: &str, cid: Cid) -> Result<DirectoryNode> {
        let bytes = self
            .read_blob(cid, BlobLength::AtMost(max_len::<DirectoryNode>()))
            .await?;
        let node = DirectoryNode::from_scale_bytes(&bytes).map_err(|e| {
            FsClientError::Serialization(format!("{path}: invalid directory node: {e}"))
        })?;
        node.validate().map_err(|e| format_error(path, e))?;
        Ok(node)
    }

    async fn read_manifest(&self, path: &str, cid: Cid) -> Result<FileManifest> {
        let bytes = self
            .read_blob(cid, BlobLength::AtMost(max_len::<FileManifest>()))
            .await?;
        let manifest = FileManifest::from_scale_bytes(&bytes).map_err(|e| {
            FsClientError::Serialization(format!("{path}: invalid file manifest: {e}"))
        })?;
        manifest.validate().map_err(|e| format_error(path, e))?;
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

/// Maximum SCALE encoded size of `T`, in bytes.
fn max_len<T: MaxEncodedLen>() -> u64 {
    T::max_encoded_len() as u64
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
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory Layer 0: blobs by CID, and the committed leaves.
    #[derive(Default)]
    struct MemoryStore {
        blobs: Mutex<HashMap<Cid, Vec<u8>>>,
        commits: Mutex<Vec<Vec<Cid>>>,
        /// Replaces the bytes returned for this CID.
        tampered: Mutex<Option<(Cid, Vec<u8>)>>,
    }

    impl MemoryStore {
        fn commits(&self) -> Vec<Vec<Cid>> {
            self.commits.lock().unwrap().clone()
        }

        fn raw_commit(&self, data: &[u8]) {
            let cid = compute_cid(data);
            self.blobs.lock().unwrap().insert(cid, data.to_vec());
            self.commits.lock().unwrap().push(vec![cid]);
        }
    }

    #[async_trait::async_trait]
    impl BlobStore for MemoryStore {
        async fn put_blob(&self, _bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
            let cid = compute_cid(data);
            self.blobs.lock().unwrap().insert(cid, data.to_vec());
            Ok(cid)
        }

        async fn get_blob(&self, cid: Cid, _length: BlobLength) -> Result<Vec<u8>> {
            if let Some((tampered, bytes)) = self.tampered.lock().unwrap().clone() {
                if tampered == cid {
                    return Ok(bytes);
                }
            }
            self.blobs
                .lock()
                .unwrap()
                .get(&cid)
                .cloned()
                .ok_or_else(|| FsClientError::StorageClient("missing blob".into()))
        }

        async fn commit(&self, _bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
            let blobs = self.blobs.lock().unwrap();
            assert!(data_roots.iter().all(|c| blobs.contains_key(c)));
            self.commits.lock().unwrap().push(data_roots);
            Ok(())
        }

        async fn last_leaf(&self, _bucket_id: BucketId) -> Result<Option<Cid>> {
            Ok(self
                .commits
                .lock()
                .unwrap()
                .last()
                .and_then(|c| c.last().copied()))
        }
    }

    const BUCKET: BucketId = 7;

    fn tree(store: &MemoryStore) -> Tree<&MemoryStore> {
        Tree::new(store, BUCKET)
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

    fn mmr_leaf(n: u8) -> storage_primitives::MmrLeaf {
        storage_primitives::MmrLeaf {
            data_root: storage_primitives::blake2_256(&[n]),
            data_size: 1,
            total_size: n as u64,
        }
    }

    fn leaf_hash(leaf: &storage_primitives::MmrLeaf) -> Cid {
        storage_primitives::blake2_256(&codec::Encode::encode(leaf))
    }

    fn mmr_proof(
        peaks: Vec<Cid>,
        leaf: storage_primitives::MmrLeaf,
        siblings: Vec<Cid>,
        path: Vec<bool>,
    ) -> storage_primitives::MmrProof {
        storage_primitives::MmrProof {
            peaks,
            leaf,
            leaf_proof: storage_primitives::MerkleProof { siblings, path },
        }
    }

    #[test]
    fn last_mmr_leaf_check_rejects_other_leaves() {
        use storage_primitives::hash_children;
        let leaves: Vec<_> = (0..4).map(mmr_leaf).collect();
        let h: Vec<Cid> = leaves.iter().map(leaf_hash).collect();

        // Three leaves: peaks [h0h1, h2].
        let peaks = vec![hash_children(h[0], h[1]), h[2]];
        let root = hash_children(peaks[0], peaks[1]);
        let last = mmr_proof(peaks.clone(), leaves[2].clone(), vec![], vec![]);
        assert!(verify_last_mmr_leaf(&last, &root, 3));
        assert!(!verify_last_mmr_leaf(&last, &root, 4));
        assert!(!verify_last_mmr_leaf(&last, &root, 0));
        let first = mmr_proof(peaks.clone(), leaves[0].clone(), vec![h[1]], vec![false]);
        assert!(storage_primitives::verify_mmr_proof(&first, &root));
        assert!(!verify_last_mmr_leaf(&first, &root, 3));

        // Four leaves: one peak.
        let left = hash_children(h[0], h[1]);
        let peak = hash_children(left, hash_children(h[2], h[3]));
        let last = mmr_proof(
            vec![peak],
            leaves[3].clone(),
            vec![h[2], left],
            vec![true, true],
        );
        assert!(verify_last_mmr_leaf(&last, &peak, 4));
        let third = mmr_proof(
            vec![peak],
            leaves[2].clone(),
            vec![h[3], left],
            vec![false, true],
        );
        assert!(storage_primitives::verify_mmr_proof(&third, &peak));
        assert!(!verify_last_mmr_leaf(&third, &peak, 4));
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
    async fn put_file_commits_content_manifest_directories_then_root() {
        let store = MemoryStore::default();
        let stat = tree(&store)
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
        let b = DirectoryNode::from_scale_bytes(&blobs[&order[2]]).unwrap();
        assert_eq!(names(&b.children), ["f.txt"]);
        let a = DirectoryNode::from_scale_bytes(&blobs[&order[3]]).unwrap();
        assert_eq!(a.find_child("b").unwrap().cid, order[2]);
        let root = DirectoryNode::from_scale_bytes(&blobs[&order[4]]).unwrap();
        assert_eq!(root.find_child("a").unwrap().cid, order[3]);

        let root = tree(&store).load_root().await.unwrap();
        assert_eq!(root.cid, Some(order[4]));

        let file = tree(&store).get_file("/a/b/f.txt").await.unwrap();
        assert_eq!(file.data, b"hello");
        assert_eq!(file.stat.content_type(), "text/plain");
        assert_eq!(file.stat.size(), 5);
        assert_eq!(file.stat.entry.size, 5);
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

    const DEFAULT: &str = file_system_primitives::DEFAULT_MIME_TYPE;

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
        let bytes = root.to_scale_bytes();
        assert!(bytes.len() > file_system_primitives::CHUNK_SIZE);
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
        store.raw_commit(&DirectoryNode::new_empty(BUCKET + 1).to_scale_bytes());
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::NotAFileSystemBucket { .. })
        ));
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
        let stat = t.put_file("/f", b"x", "", vec![]).await.unwrap();
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
        store.raw_commit(&root.to_scale_bytes());
    }

    #[tokio::test]
    async fn metadata_reads_reject_blobs_over_the_max_encoded_size() {
        let store = MemoryStore::default();
        store.raw_commit(&vec![0u8; DirectoryNode::max_encoded_len() + 1]);
        assert!(matches!(
            tree(&store).load_root().await,
            Err(FsClientError::NotAFileSystemBucket { bucket_id: BUCKET, reason })
                if reason.contains("larger than a directory node")
        ));

        let store = MemoryStore::default();
        commit_file_with_manifest(&store, &vec![0u8; FileManifest::max_encoded_len() + 1]);
        assert!(matches!(
            tree(&store).stat("/f").await,
            Err(FsClientError::Serialization(e)) if e.contains("expected AtMost")
        ));
    }

    #[tokio::test]
    async fn stat_rejects_a_manifest_whose_size_disagrees_with_its_chunks() {
        let store = MemoryStore::default();
        let mut manifest = FileManifest::for_content(BUCKET, b"x", DEFAULT, vec![]).unwrap();
        manifest.total_size = 1 << 20;
        commit_file_with_manifest(&store, &manifest.to_scale_bytes());
        assert!(matches!(
            tree(&store).stat("/f").await,
            Err(FsClientError::Serialization(e)) if e.contains("Invalid file manifest")
        ));
    }
}
