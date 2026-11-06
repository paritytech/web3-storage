// SPDX-License-Identifier: Apache-2.0

//! A file system and S3-style objects stored in a Layer 0 bucket.
//!
//! A drive is a Layer 0 bucket of `pallet-storage-provider`, identified by
//! its bucket id. Directories, file manifests and file contents are blobs in
//! the bucket, and the root directory is the bucket's last MMR leaf. The
//! [`format`](mod@format) module defines these blobs and their limits,
//! [`tree`] reads and writes them, and [`s3`] maps object keys onto the same
//! tree.
//!
//! This crate makes no chain calls. Create the bucket and manage its members
//! with `storage_client::AdminClient`, and checkpoint it with
//! `storage_client::CheckpointManager`.
//!
//! # Example
//!
//! ```ignore
//! use storage_formats::{s3::Objects, Tree};
//! use storage_client::{Signer, StorageUserClient};
//!
//! let store = StorageUserClient::for_provider("http://127.0.0.1:3333", Signer::from_seed("//Alice")?)?;
//!
//! let tree = Tree::new(&store, bucket_id);
//! tree.put_file("/documents/report.pdf", &pdf, "application/pdf", Vec::new()).await?;
//! let entries = tree.list("/documents").await?;
//! let file = tree.get_file("/documents/report.pdf").await?;
//!
//! let objects = Objects::new(Tree::new(&store, bucket_id));
//! objects.put_object("photos/cat.jpg", &jpeg, Default::default()).await?;
//! ```

pub mod format;
#[cfg(feature = "http")]
mod http;
pub mod s3;
pub mod tree;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm;

#[cfg(test)]
mod testing;

use crate::format::{Cid, FileSystemError};
use thiserror::Error;

pub use storage_primitives::BucketId;
pub use tree::{BlobLength, BlobStore, EmptyParents, FileContent, FileStat, Root, Tree};

/// Errors of the file tree and its blob store. The wasm build throws them as
/// JS errors with the variant name as `code`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsClientError {
    /// The path does not start with `/`, has an empty name (trailing or
    /// double `/`), or a name that breaks
    /// [`crate::format::validate_entry_name`]. Also `/` where an
    /// entry is needed.
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    /// Nothing exists at the path.
    #[error("Path not found: {0}")]
    PathNotFound(String),

    /// An entry already exists at the path.
    #[error("Entry already exists: {0}")]
    EntryExists(String),

    /// A name on the path, before the last one, is a file.
    #[error("Not a directory: {0}")]
    NotADirectory(String),

    /// The path is a directory where the operation needs a file.
    #[error("Not a file: {0}")]
    NotAFile(String),

    /// The directory has entries; delete them first.
    #[error("Directory not empty: {0}")]
    DirectoryNotEmpty(String),

    /// A rule of the format rejected the write: the directory is full, the
    /// file is too large, or the metadata is over its bounds or repeats a
    /// key. `source` says which.
    #[error("{path}: {source}")]
    Format {
        /// Path of the entry being written.
        path: String,
        /// The rule that failed.
        source: FileSystemError,
    },

    /// The bucket's last MMR leaf is not a root directory of this drive.
    /// Something other than this crate committed to the bucket.
    #[error("Bucket {bucket_id} is not a file system bucket: {reason}")]
    NotAFileSystemBucket {
        /// The bucket that was read.
        bucket_id: BucketId,
        /// What is wrong with the last leaf.
        reason: String,
    },

    /// A directory node or file manifest has a newer format version than
    /// this code reads. Update the client.
    #[error("Unsupported file-system format version {0}; update the client")]
    UnsupportedFormatVersion(u8),

    /// The provider's MMR proof is not a proof of the last leaf of its
    /// commitment. A commit by another writer between the two requests also
    /// causes it; retry the operation.
    #[error("Invalid MMR proof for the last leaf of bucket {0}")]
    InvalidMmrProof(BucketId),

    /// A blob below the root is not what its parent entry says: the wrong
    /// length, not a valid directory node or manifest, or a manifest whose
    /// chunk list does not match its size.
    #[error("Invalid blob at {path}: {reason}")]
    InvalidBlob {
        /// Path of the entry whose blob was read.
        path: String,
        /// What is wrong with the blob.
        reason: String,
    },

    /// A blob's data root is not the CID that references it.
    #[error("CID mismatch: expected {expected:?}, got {got:?}")]
    CidMismatch {
        /// The CID that references the blob.
        expected: Cid,
        /// The data root of the bytes read or uploaded.
        got: Cid,
    },

    /// A provider request failed; see [`storage_client::ClientError`]. A 403
    /// ([`storage_client::ClientError::HttpStatus`]) means the signer lacks
    /// the bucket role the request needs.
    #[cfg(feature = "http")]
    #[error(transparent)]
    Provider(#[from] storage_client::ClientError),

    /// A blob store other than the HTTP one failed. For the JS store of the
    /// wasm build, the message is the message of the JS error, for example a
    /// provider status.
    #[error("Blob store error: {0}")]
    Store(String),
}

pub type Result<T> = std::result::Result<T, FsClientError>;
