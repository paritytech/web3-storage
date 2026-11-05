// SPDX-License-Identifier: Apache-2.0

//! A file system and S3-style objects stored in a Layer 0 bucket.
//!
//! A drive is a Layer 0 bucket of `pallet-storage-provider`, identified by
//! its bucket id. Directories, file manifests and file contents are blobs in
//! the bucket, and the root directory is the bucket's last MMR leaf. The
//! [`tree`] module contains the format, the read and write logic and the
//! limits; the [`s3`] module maps object keys onto the same tree.
//!
//! This crate makes no chain calls. Create the bucket and manage its members
//! with `storage_client::AdminClient`, and checkpoint it with
//! `storage_client::CheckpointManager`.
//!
//! # Example
//!
//! ```ignore
//! use bucket_tree::{s3::Objects, Tree};
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

use crate::format::Cid;
use thiserror::Error;

pub use storage_primitives::BucketId;
pub use tree::{BlobLength, BlobStore, EmptyParents, FileContent, FileStat, Root, Tree};

/// File system client errors
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsClientError {
    #[error("Storage client error: {0}")]
    StorageClient(String),

    #[error("Path not found: {0}")]
    PathNotFound(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Entry already exists: {0}")]
    EntryExists(String),

    #[error("Not a directory: {0}")]
    NotADirectory(String),

    #[error("Not a file: {0}")]
    NotAFile(String),

    /// The directory has entries; delete them first.
    #[error("Directory not empty: {0}")]
    DirectoryNotEmpty(String),

    /// The bucket's last MMR leaf is not a root directory of this drive.
    /// Something other than the file system or S3 clients committed to the
    /// bucket.
    #[error("Bucket {bucket_id} is not a file system bucket: {reason}")]
    NotAFileSystemBucket {
        /// The bucket that was read.
        bucket_id: BucketId,
        /// What is wrong with the last leaf.
        reason: String,
    },

    /// The provider's MMR proof is not a proof of the last leaf of its
    /// commitment. A commit by another writer between the two requests also
    /// causes it; retry the operation.
    #[error("Invalid MMR proof for the last leaf of bucket {0}")]
    InvalidMmrProof(BucketId),

    #[cfg(feature = "http")]
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Blockchain error: {0}")]
    Blockchain(String),

    #[error("Event not found in transaction")]
    EventNotFound,

    #[error("Bounded collection overflow")]
    BoundedOverflow,

    #[error("No signer configured")]
    NoSigner,

    #[error("Configuration error: {0}")]
    Config(String),

    /// A blob's data root is not the CID that references it.
    #[error("CID mismatch: expected {expected:?}, got {got:?}")]
    CidMismatch {
        /// The CID that references the blob.
        expected: Cid,
        /// The data root of the bytes read or uploaded.
        got: Cid,
    },
}

#[cfg(feature = "http")]
impl From<storage_client::ClientError> for FsClientError {
    fn from(e: storage_client::ClientError) -> Self {
        use storage_client::ClientError;
        match e {
            ClientError::Http(e) => Self::Network(e),
            ClientError::Chain(message) => Self::Blockchain(message),
            ClientError::Config(message) => Self::Config(message),
            ClientError::Serialization(message) => Self::Serialization(message),
            other => Self::StorageClient(other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, FsClientError>;
